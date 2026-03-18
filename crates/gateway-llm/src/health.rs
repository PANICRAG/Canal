//! Provider health tracking with circuit breaker pattern
//!
//! This module implements a circuit breaker for LLM providers to prevent
//! cascading failures. Each provider transitions through three states:
//!
//! - **Closed**: Normal operation. Requests flow through. Failures are counted.
//! - **Open**: Provider is considered unhealthy. Requests are rejected until
//!   the cooldown period expires.
//! - **HalfOpen**: After cooldown, a limited number of probe requests are
//!   allowed. If enough succeed, the circuit closes. If any fail, it reopens.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

// ============================================================================
// Circuit Breaker State
// ============================================================================

/// Circuit breaker state for a provider.
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    /// Normal operation -- requests flow through.
    Closed,
    /// Provider is unhealthy -- requests are blocked until cooldown expires.
    Open,
    /// Cooldown expired -- limited probe requests are allowed.
    HalfOpen,
}

impl CircuitState {
    /// Return a string label suitable for serialisation.
    pub fn as_str(&self) -> &'static str {
        match self {
            CircuitState::Closed => "closed",
            CircuitState::Open => "open",
            CircuitState::HalfOpen => "half_open",
        }
    }
}

// ============================================================================
// Per-provider health state (internal)
// ============================================================================

/// Internal mutable health state for a single provider.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub state: CircuitState,
    pub consecutive_failures: u32,
    pub consecutive_successes_in_half_open: u32,
    pub last_failure: Option<Instant>,
    pub last_success: Option<Instant>,
    pub total_requests: u64,
    pub total_failures: u64,
    pub avg_latency_ms: f64,
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            consecutive_successes_in_half_open: 0,
            last_failure: None,
            last_success: None,
            total_requests: 0,
            total_failures: 0,
            avg_latency_ms: 0.0,
        }
    }
}

// ============================================================================
// Configuration
// ============================================================================

/// Configuration knobs for the circuit breaker.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Number of consecutive failures before the circuit opens.
    pub failure_threshold: u32,
    /// Seconds to wait in the Open state before transitioning to HalfOpen.
    pub cooldown_seconds: u64,
    /// Number of consecutive successes in HalfOpen required to close the circuit.
    pub success_to_recover: u32,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            cooldown_seconds: 30,
            success_to_recover: 2,
        }
    }
}

// ============================================================================
// Serialisable snapshot
// ============================================================================

/// A point-in-time, serialisable snapshot of a provider's health.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealthSnapshot {
    pub provider: String,
    pub state: String,
    pub consecutive_failures: u32,
    pub total_requests: u64,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
}

impl ProviderHealthSnapshot {
    fn from_health(provider: &str, health: &ProviderHealth) -> Self {
        let success_rate = if health.total_requests > 0 {
            1.0 - (health.total_failures as f64 / health.total_requests as f64)
        } else {
            1.0
        };
        Self {
            provider: provider.to_string(),
            state: health.state.as_str().to_string(),
            consecutive_failures: health.consecutive_failures,
            total_requests: health.total_requests,
            success_rate,
            avg_latency_ms: health.avg_latency_ms,
        }
    }
}

// ============================================================================
// HealthTracker
// ============================================================================

/// Thread-safe health tracker that manages circuit breaker state for every
/// registered LLM provider.
///
/// All methods take `&self` and use an internal `std::sync::RwLock` so that the
/// tracker can be shared via `Arc<HealthTracker>` without requiring `&mut self`.
pub struct HealthTracker {
    states: RwLock<HashMap<String, ProviderHealth>>,
    config: HealthConfig,
}

impl HealthTracker {
    /// Create a new tracker with the given configuration.
    pub fn new(config: HealthConfig) -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Create a new tracker using [`HealthConfig::default`].
    pub fn with_default_config() -> Self {
        Self::new(HealthConfig::default())
    }

    // -- Recording outcomes ---------------------------------------------------

    /// Record a successful request to `provider`.
    ///
    /// State transitions:
    /// - **Closed**: resets consecutive failures.
    /// - **HalfOpen**: increments successes; if `>= success_to_recover` the
    ///   circuit transitions to Closed.
    /// - **Open**: should not normally happen (callers check `is_healthy` first)
    ///   but is handled gracefully by resetting to Closed.
    pub fn record_success(&self, provider: &str) {
        let mut states = self.states.write().unwrap_or_else(|e| e.into_inner());
        let health = states
            .entry(provider.to_string())
            .or_insert_with(ProviderHealth::default);

        health.total_requests += 1;
        health.last_success = Some(Instant::now());

        match health.state {
            CircuitState::Closed => {
                health.consecutive_failures = 0;
            }
            CircuitState::HalfOpen => {
                health.consecutive_successes_in_half_open += 1;
                if health.consecutive_successes_in_half_open >= self.config.success_to_recover {
                    health.state = CircuitState::Closed;
                    health.consecutive_failures = 0;
                    health.consecutive_successes_in_half_open = 0;
                    tracing::info!(
                        provider = provider,
                        "Circuit breaker closed -- provider recovered"
                    );
                }
            }
            CircuitState::Open => {
                // Defensive: treat as recovery.
                health.state = CircuitState::Closed;
                health.consecutive_failures = 0;
                health.consecutive_successes_in_half_open = 0;
            }
        }
    }

    /// Record a successful request and update the running average latency.
    pub fn record_success_with_latency(&self, provider: &str, latency: Duration) {
        {
            let mut states = self.states.write().unwrap_or_else(|e| e.into_inner());
            let health = states
                .entry(provider.to_string())
                .or_insert_with(ProviderHealth::default);

            // Exponential moving average (alpha = 0.3).
            let latency_ms = latency.as_secs_f64() * 1000.0;
            if health.total_requests == 0 {
                health.avg_latency_ms = latency_ms;
            } else {
                health.avg_latency_ms = health.avg_latency_ms * 0.7 + latency_ms * 0.3;
            }
        }
        // Delegate the rest of the bookkeeping to `record_success`.
        self.record_success(provider);
    }

    /// Record a failed request to `provider`.
    ///
    /// State transitions:
    /// - **Closed**: increments failures; if `>= failure_threshold` the circuit
    ///   transitions to Open.
    /// - **HalfOpen**: immediately reopens the circuit.
    /// - **Open**: increments the failure counter (informational).
    pub fn record_failure(&self, provider: &str) {
        let mut states = self.states.write().unwrap_or_else(|e| e.into_inner());
        let health = states
            .entry(provider.to_string())
            .or_insert_with(ProviderHealth::default);

        health.total_requests += 1;
        health.total_failures += 1;
        health.consecutive_failures += 1;
        health.last_failure = Some(Instant::now());

        match health.state {
            CircuitState::Closed => {
                if health.consecutive_failures >= self.config.failure_threshold {
                    health.state = CircuitState::Open;
                    tracing::warn!(
                        provider = provider,
                        failures = health.consecutive_failures,
                        "Circuit breaker opened -- provider marked unhealthy"
                    );
                }
            }
            CircuitState::HalfOpen => {
                // Any failure during probing reopens the circuit.
                health.state = CircuitState::Open;
                health.consecutive_successes_in_half_open = 0;
                tracing::warn!(
                    provider = provider,
                    "Circuit breaker reopened -- probe request failed"
                );
            }
            CircuitState::Open => {
                // Already open; nothing to transition.
            }
        }
    }

    // -- Querying health ------------------------------------------------------

    /// Check whether `provider` should be considered healthy enough to receive
    /// traffic.
    ///
    /// - **Closed**: returns `true`.
    /// - **Open**: if the cooldown has expired the state is moved to HalfOpen
    ///   and `true` is returned (allowing a probe request). Otherwise `false`.
    /// - **HalfOpen**: returns `true` (probe requests are allowed).
    ///
    /// For providers that have never been seen, returns `true` (optimistic).
    pub fn is_healthy(&self, provider: &str) -> bool {
        // Fast path: read lock.
        {
            let states = self.states.read().unwrap_or_else(|e| e.into_inner());
            match states.get(provider) {
                None => return true,
                Some(h) if h.state == CircuitState::Closed => return true,
                Some(h) if h.state == CircuitState::HalfOpen => return true,
                _ => {}
            }
        }

        // Slow path: the provider is Open -- check whether cooldown has
        // expired and, if so, transition to HalfOpen.
        let mut states = self.states.write().unwrap_or_else(|e| e.into_inner());
        let health = match states.get_mut(provider) {
            Some(h) => h,
            None => return true, // race: removed between read and write
        };

        if health.state != CircuitState::Open {
            // Another thread transitioned it while we waited for the write lock.
            return health.state != CircuitState::Open;
        }

        let cooldown = Duration::from_secs(self.config.cooldown_seconds);
        let expired = health
            .last_failure
            .map(|t| t.elapsed() >= cooldown)
            .unwrap_or(true);

        if expired {
            health.state = CircuitState::HalfOpen;
            health.consecutive_successes_in_half_open = 0;
            tracing::info!(
                provider = provider,
                "Circuit breaker half-open -- allowing probe requests"
            );
            true
        } else {
            false
        }
    }

    // -- Snapshots ------------------------------------------------------------

    /// Return a snapshot of every tracked provider's health.
    pub fn get_all_status(&self) -> HashMap<String, ProviderHealthSnapshot> {
        let states = self.states.read().unwrap_or_else(|e| e.into_inner());
        states
            .iter()
            .map(|(name, health)| {
                (
                    name.clone(),
                    ProviderHealthSnapshot::from_health(name, health),
                )
            })
            .collect()
    }

    /// Return a snapshot for a single provider, if it has been tracked.
    pub fn get_provider_status(&self, provider: &str) -> Option<ProviderHealthSnapshot> {
        let states = self.states.read().unwrap_or_else(|e| e.into_inner());
        states
            .get(provider)
            .map(|h| ProviderHealthSnapshot::from_health(provider, h))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// Helper: build a tracker with a very short cooldown so tests run fast.
    fn fast_tracker() -> HealthTracker {
        HealthTracker::new(HealthConfig {
            failure_threshold: 3,
            cooldown_seconds: 0, // immediate cooldown for testing
            success_to_recover: 2,
        })
    }

    // -- Basic state transitions ----------------------------------------------

    #[test]
    fn test_initial_state_is_healthy() {
        let tracker = fast_tracker();
        assert!(tracker.is_healthy("anthropic"));
    }

    #[test]
    fn test_stays_closed_below_threshold() {
        let tracker = fast_tracker();
        tracker.record_failure("anthropic");
        tracker.record_failure("anthropic");
        // 2 failures, threshold is 3 -- still closed.
        assert!(tracker.is_healthy("anthropic"));
        let snap = tracker.get_provider_status("anthropic").unwrap();
        assert_eq!(snap.state, "closed");
        assert_eq!(snap.consecutive_failures, 2);
    }

    #[test]
    fn test_opens_at_threshold() {
        let tracker = HealthTracker::new(HealthConfig {
            failure_threshold: 3,
            cooldown_seconds: 9999, // effectively never expire in this test
            success_to_recover: 2,
        });

        for _ in 0..3 {
            tracker.record_failure("anthropic");
        }
        // With a very long cooldown the circuit stays open.
        assert!(!tracker.is_healthy("anthropic"));
        let snap = tracker.get_provider_status("anthropic").unwrap();
        assert_eq!(snap.state, "open");
    }

    #[test]
    fn test_open_to_half_open_after_cooldown() {
        let tracker = fast_tracker(); // cooldown = 0s

        for _ in 0..3 {
            tracker.record_failure("openai");
        }

        // cooldown_seconds == 0, so `is_healthy` should transition to HalfOpen.
        assert!(tracker.is_healthy("openai"));
        let snap = tracker.get_provider_status("openai").unwrap();
        assert_eq!(snap.state, "half_open");
    }

    #[test]
    fn test_half_open_to_closed_after_enough_successes() {
        let tracker = fast_tracker();

        // Open the circuit.
        for _ in 0..3 {
            tracker.record_failure("google");
        }
        // Transition to HalfOpen.
        assert!(tracker.is_healthy("google"));

        // Probe successes.
        tracker.record_success("google");
        let snap = tracker.get_provider_status("google").unwrap();
        assert_eq!(snap.state, "half_open"); // 1 success, need 2

        tracker.record_success("google");
        let snap = tracker.get_provider_status("google").unwrap();
        assert_eq!(snap.state, "closed"); // recovered
        assert_eq!(snap.consecutive_failures, 0);
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let tracker = HealthTracker::new(HealthConfig {
            failure_threshold: 3,
            cooldown_seconds: 0,
            success_to_recover: 2,
        });

        // Open the circuit.
        for _ in 0..3 {
            tracker.record_failure("anthropic");
        }
        // Transition to HalfOpen.
        assert!(tracker.is_healthy("anthropic"));

        // One probe success, then a failure.
        tracker.record_success("anthropic");
        tracker.record_failure("anthropic");
        let snap = tracker.get_provider_status("anthropic").unwrap();
        assert_eq!(snap.state, "open");
    }

    #[test]
    fn test_success_resets_consecutive_failures_in_closed() {
        let tracker = fast_tracker();

        tracker.record_failure("anthropic");
        tracker.record_failure("anthropic");
        tracker.record_success("anthropic");

        let snap = tracker.get_provider_status("anthropic").unwrap();
        assert_eq!(snap.state, "closed");
        assert_eq!(snap.consecutive_failures, 0);
    }

    // -- Counters & snapshots -------------------------------------------------

    #[test]
    fn test_total_requests_and_failures() {
        let tracker = fast_tracker();

        tracker.record_success("a");
        tracker.record_success("a");
        tracker.record_failure("a");

        let snap = tracker.get_provider_status("a").unwrap();
        assert_eq!(snap.total_requests, 3);
        assert!((snap.success_rate - (2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn test_get_all_status() {
        let tracker = fast_tracker();
        tracker.record_success("a");
        tracker.record_success("b");

        let all = tracker.get_all_status();
        assert_eq!(all.len(), 2);
        assert!(all.contains_key("a"));
        assert!(all.contains_key("b"));
    }

    #[test]
    fn test_unknown_provider_returns_none() {
        let tracker = fast_tracker();
        assert!(tracker.get_provider_status("nope").is_none());
    }

    // -- Latency tracking -----------------------------------------------------

    #[test]
    fn test_latency_tracking() {
        let tracker = fast_tracker();
        tracker.record_success_with_latency("a", Duration::from_millis(100));
        let snap = tracker.get_provider_status("a").unwrap();
        assert!((snap.avg_latency_ms - 100.0).abs() < 1.0);

        // Second reading should move the average (EMA alpha=0.3).
        tracker.record_success_with_latency("a", Duration::from_millis(200));
        let snap = tracker.get_provider_status("a").unwrap();
        // EMA: 100*0.7 + 200*0.3 = 130
        assert!((snap.avg_latency_ms - 130.0).abs() < 1.0);
    }

    // -- Thread safety --------------------------------------------------------

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;

        let tracker = Arc::new(fast_tracker());
        let mut handles = Vec::new();

        for i in 0..10 {
            let t = Arc::clone(&tracker);
            handles.push(thread::spawn(move || {
                let provider = format!("provider-{}", i % 3);
                for _ in 0..100 {
                    t.record_success(&provider);
                    t.is_healthy(&provider);
                    t.record_failure(&provider);
                    t.is_healthy(&provider);
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Smoke check: we should have entries for the three providers.
        let all = tracker.get_all_status();
        assert_eq!(all.len(), 3);
    }

    // -- Serialisation --------------------------------------------------------

    #[test]
    fn test_snapshot_serialises_to_json() {
        let tracker = fast_tracker();
        tracker.record_success("anthropic");

        let snap = tracker.get_provider_status("anthropic").unwrap();
        let json = serde_json::to_string(&snap).expect("serialise");
        assert!(json.contains("\"provider\":\"anthropic\""));
        assert!(json.contains("\"state\":\"closed\""));
    }
}
