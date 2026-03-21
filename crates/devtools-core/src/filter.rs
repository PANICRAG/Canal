//! Query filters and update types for traces and observations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{ObservationStatus, TraceStatus};

/// Filter for listing traces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceFilter {
    /// Filter by project ID.
    pub project_id: Option<String>,
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by trace status.
    pub status: Option<TraceStatus>,
    /// Filter by user ID.
    pub user_id: Option<String>,
    /// Filter by tag (any match).
    pub tag: Option<String>,
    /// Filter by name (substring match).
    pub name: Option<String>,
    /// Start time lower bound.
    pub start_after: Option<DateTime<Utc>>,
    /// Start time upper bound.
    pub start_before: Option<DateTime<Utc>>,
    /// Maximum results to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: usize,
}

impl Default for TraceFilter {
    fn default() -> Self {
        Self {
            project_id: None,
            session_id: None,
            status: None,
            user_id: None,
            tag: None,
            name: None,
            start_after: None,
            start_before: None,
            limit: default_limit(),
            offset: 0,
        }
    }
}

/// Filter for metrics aggregation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsFilter {
    /// Filter by project ID.
    pub project_id: Option<String>,
    /// Start of time range.
    pub start_time: Option<DateTime<Utc>>,
    /// End of time range.
    pub end_time: Option<DateTime<Utc>>,
}

/// Partial update for a trace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraceUpdate {
    pub status: Option<TraceStatus>,
    pub end_time: Option<DateTime<Utc>>,
    pub output: Option<serde_json::Value>,
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Partial update for an observation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObservationUpdate {
    pub status: Option<ObservationStatus>,
    pub end_time: Option<DateTime<Utc>>,
    pub output: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    /// For generation updates: token counts and cost.
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub cost_usd: Option<f64>,
}

fn default_limit() -> usize {
    50
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_filter_defaults() {
        let filter = TraceFilter::default();
        assert_eq!(filter.limit, 50);
        assert_eq!(filter.offset, 0);
        assert!(filter.project_id.is_none());
        assert!(filter.status.is_none());
    }

    #[test]
    fn test_metrics_filter_time_range() {
        let now = Utc::now();
        let filter = MetricsFilter {
            project_id: Some("proj-1".into()),
            start_time: Some(now - chrono::Duration::hours(1)),
            end_time: Some(now),
        };
        assert!(filter.start_time.unwrap() < filter.end_time.unwrap());
    }

    #[test]
    fn test_trace_filter_serde() {
        let filter = TraceFilter {
            project_id: Some("proj-1".into()),
            status: Some(TraceStatus::Running),
            limit: 20,
            ..Default::default()
        };
        let json = serde_json::to_string(&filter).unwrap();
        let rt: TraceFilter = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.project_id, Some("proj-1".into()));
        assert_eq!(rt.limit, 20);
    }
}
