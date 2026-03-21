//! Distributed trace context for cross-service observability.
//!
//! Propagated in gRPC metadata (tonic interceptor) to link
//! observations from different services into a single trace.

use serde::{Deserialize, Serialize};

/// Trace context propagated across service boundaries.
///
/// Compatible with W3C Trace Context headers for interop with
/// external tracing systems (Jaeger, Zipkin, OpenTelemetry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    /// Unique trace identifier (shared across all spans in a request)
    pub trace_id: String,
    /// Unique span identifier for the current operation
    pub span_id: String,
    /// Parent span ID (None for root spans)
    pub parent_span_id: Option<String>,
    /// Service that created this span
    pub service_name: String,
}

impl TraceContext {
    /// Create a new root trace context.
    pub fn new_root(service_name: impl Into<String>) -> Self {
        Self {
            trace_id: uuid_v4(),
            span_id: uuid_v4(),
            parent_span_id: None,
            service_name: service_name.into(),
        }
    }

    /// Create a child span from this context.
    pub fn child(&self, service_name: impl Into<String>) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: uuid_v4(),
            parent_span_id: Some(self.span_id.clone()),
            service_name: service_name.into(),
        }
    }
}

/// Generate a unique trace/span ID using timestamp + randomized hash.
///
/// Not a conformant UUID v4, but collision-resistant: combines nanosecond
/// timestamp with entropy from `RandomState` (seeded from OS randomness).
fn uuid_v4() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    // RandomState is seeded from OS randomness on construction
    let random_bits = RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", now.as_nanos() as u64, random_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root_trace() {
        let ctx = TraceContext::new_root("gateway-api");
        assert_eq!(ctx.service_name, "gateway-api");
        assert!(ctx.parent_span_id.is_none());
        assert!(!ctx.trace_id.is_empty());
        assert!(!ctx.span_id.is_empty());
    }

    #[test]
    fn test_child_span() {
        let root = TraceContext::new_root("gateway-api");
        let child = root.child("tool-service");
        assert_eq!(child.trace_id, root.trace_id);
        assert_eq!(child.parent_span_id, Some(root.span_id.clone()));
        assert_eq!(child.service_name, "tool-service");
        assert_ne!(child.span_id, root.span_id);
    }
}
