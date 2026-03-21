//! ScreenMonitor — adaptive background polling with change notification.
//!
//! Polls the screen at adaptive intervals and broadcasts change events.
//! Uses the shared ScreenChangeDetector instance (this is the ONLY consumer).
//!
//! Polling cost: <1% CPU idle, <5% active. pHash is ~5ms per capture.
//! Memory: ~1KB for 10 MonitoredState entries (pHash + metadata, no screenshots).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinHandle;

use crate::change_detector::ScreenChangeDetector;
use crate::monitor_events::{ChangeType, MonitoredState, ScreenChangeEvent};
use crate::phash::{compute_phash, hash_similarity};
use crate::screen_controller::ScreenController;
use crate::types::ContextInfo;

/// Configuration for the screen monitor.
#[derive(Debug, Clone)]
pub struct ScreenMonitorConfig {
    /// Polling interval when idle (no tool calls for 30s). Default: 5000ms.
    pub idle_interval_ms: u64,
    /// Polling interval when active (recent tool calls). Default: 1000ms.
    pub active_interval_ms: u64,
    /// Burst polling after an action. Default: 200ms.
    pub post_action_interval_ms: u64,
    /// Number of burst polls after an action. Default: 3.
    pub post_action_burst_count: u32,
    /// pHash similarity threshold below which a change is detected. Default: 0.85.
    pub change_threshold: f32,
    /// Maximum number of MonitoredState entries to keep. Default: 10.
    pub history_size: usize,
}

impl Default for ScreenMonitorConfig {
    fn default() -> Self {
        Self {
            idle_interval_ms: 5000,
            active_interval_ms: 1000,
            post_action_interval_ms: 200,
            post_action_burst_count: 3,
            change_threshold: 0.85,
            history_size: 10,
        }
    }
}

/// Adaptive screen monitor with background polling and change broadcasting.
///
/// Only consumer of the shared `ScreenChangeDetector`. All other components
/// (pipeline, chain executor) use local pHash baselines.
pub struct ScreenMonitor {
    controller: Arc<dyn ScreenController>,
    _change_detector: Arc<ScreenChangeDetector>,
    config: ScreenMonitorConfig,
    running: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    burst_remaining: Arc<RwLock<u32>>,
    last_state: Arc<RwLock<Option<MonitoredState>>>,
    history: Arc<RwLock<Vec<MonitoredState>>>,
    tx: broadcast::Sender<ScreenChangeEvent>,
}

impl ScreenMonitor {
    /// Create a new screen monitor.
    pub fn new(
        controller: Arc<dyn ScreenController>,
        change_detector: Arc<ScreenChangeDetector>,
        config: ScreenMonitorConfig,
    ) -> Self {
        let (tx, _) = broadcast::channel(32);
        Self {
            controller,
            _change_detector: change_detector,
            config,
            running: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(false)),
            burst_remaining: Arc::new(RwLock::new(0)),
            last_state: Arc::new(RwLock::new(None)),
            history: Arc::new(RwLock::new(Vec::new())),
            tx,
        }
    }

    /// Start background polling. Returns the polling task handle.
    pub async fn start(&self) -> JoinHandle<()> {
        self.running.store(true, Ordering::SeqCst);

        let controller = self.controller.clone();
        let config = self.config.clone();
        let running = self.running.clone();
        let active = self.active.clone();
        let burst_remaining = self.burst_remaining.clone();
        let last_state = self.last_state.clone();
        let history = self.history.clone();
        let tx = self.tx.clone();
        let threshold = config.change_threshold;
        let history_size = config.history_size;

        tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                // Determine interval
                let interval = {
                    let burst = *burst_remaining.read().await;
                    if burst > 0 {
                        *burst_remaining.write().await -= 1;
                        Duration::from_millis(config.post_action_interval_ms)
                    } else if active.load(Ordering::SeqCst) {
                        Duration::from_millis(config.active_interval_ms)
                    } else {
                        Duration::from_millis(config.idle_interval_ms)
                    }
                };

                tokio::time::sleep(interval).await;

                if !running.load(Ordering::SeqCst) {
                    break;
                }

                // Capture and compare
                let capture = match controller.capture().await {
                    Ok(cap) => cap,
                    Err(_) => continue,
                };

                let current_hash = compute_phash(&capture.base64);
                let context = controller.context_info();
                let now = Instant::now();

                let new_state = MonitoredState {
                    phash: current_hash,
                    display_width: capture.display_width,
                    display_height: capture.display_height,
                    context: context.clone(),
                    timestamp: now,
                };

                // Compare with last state
                let last = last_state.read().await.clone();
                if let Some(ref prev) = last {
                    let similarity = hash_similarity(prev.phash, current_hash);
                    let visual_changed = similarity < threshold;
                    let context_changed = Self::context_differs(&prev.context, &context);

                    if visual_changed || context_changed {
                        let change_type = match (visual_changed, context_changed) {
                            (true, true) => ChangeType::FullChange { similarity },
                            (false, true) => ChangeType::ContextChange,
                            (true, false) => ChangeType::ContentUpdate { similarity },
                            (false, false) => unreachable!(),
                        };

                        let event = ScreenChangeEvent {
                            change_type,
                            before_phash: prev.phash,
                            after_phash: current_hash,
                            before_context: prev.context.clone(),
                            after_context: context,
                            similarity,
                            detected_at: now,
                        };

                        let _ = tx.send(event);
                    }
                }

                // Update state
                *last_state.write().await = Some(new_state.clone());

                // Add to history
                let mut hist = history.write().await;
                hist.push(new_state);
                if hist.len() > history_size {
                    hist.remove(0);
                }
            }
        })
    }

    /// Stop the background polling loop.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Subscribe to screen change events.
    pub fn subscribe(&self) -> broadcast::Receiver<ScreenChangeEvent> {
        self.tx.subscribe()
    }

    /// Signal that an action was just performed — triggers burst polling.
    pub fn signal_action_performed(&self) {
        let burst = self.burst_remaining.clone();
        let count = self.config.post_action_burst_count;
        tokio::spawn(async move {
            *burst.write().await = count;
        });
    }

    /// Set whether the monitor is in active mode (faster polling).
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::SeqCst);
    }

    /// Get the current monitored state (if any).
    pub async fn current_state(&self) -> Option<MonitoredState> {
        self.last_state.read().await.clone()
    }

    /// Check if the monitor is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get the monitor's configuration.
    pub fn config(&self) -> &ScreenMonitorConfig {
        &self.config
    }

    /// Compare two optional ContextInfo values for differences.
    fn context_differs(a: &Option<ContextInfo>, b: &Option<ContextInfo>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => a.url != b.url || a.title != b.title || a.app_name != b.app_name,
            (None, Some(_)) | (Some(_), None) => true,
            (None, None) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_detector::ChangeDetectionConfig;
    use crate::NoopScreenController;

    fn make_monitor() -> ScreenMonitor {
        let controller = Arc::new(NoopScreenController::new());
        let detector = Arc::new(ScreenChangeDetector::new(
            controller.clone(),
            ChangeDetectionConfig::default(),
        ));
        ScreenMonitor::new(controller, detector, ScreenMonitorConfig::default())
    }

    #[test]
    fn test_config_defaults() {
        let config = ScreenMonitorConfig::default();
        assert_eq!(config.idle_interval_ms, 5000);
        assert_eq!(config.active_interval_ms, 1000);
        assert_eq!(config.post_action_interval_ms, 200);
        assert_eq!(config.post_action_burst_count, 3);
        assert!((config.change_threshold - 0.85).abs() < f32::EPSILON);
        assert_eq!(config.history_size, 10);
    }

    #[test]
    fn test_monitor_not_running_initially() {
        let monitor = make_monitor();
        assert!(!monitor.is_running());
    }

    #[test]
    fn test_subscribe_returns_receiver() {
        let monitor = make_monitor();
        let _rx = monitor.subscribe();
        // Should not panic
    }

    #[tokio::test]
    async fn test_current_state_initially_none() {
        let monitor = make_monitor();
        assert!(monitor.current_state().await.is_none());
    }

    #[tokio::test]
    async fn test_stop_sets_running_false() {
        let monitor = make_monitor();
        // Start sets running to true
        let handle = monitor.start().await;
        assert!(monitor.is_running());

        // Stop sets running to false
        monitor.stop();
        // Give the task time to exit
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!monitor.is_running());
        let _ = handle;
    }

    #[test]
    fn test_set_active() {
        let monitor = make_monitor();
        assert!(!monitor.active.load(Ordering::SeqCst));
        monitor.set_active(true);
        assert!(monitor.active.load(Ordering::SeqCst));
        monitor.set_active(false);
        assert!(!monitor.active.load(Ordering::SeqCst));
    }

    #[test]
    fn test_context_differs() {
        let a = Some(ContextInfo {
            url: Some("https://a.com".into()),
            title: Some("A".into()),
            app_name: None,
            interactive_elements: None,
        });
        let b = Some(ContextInfo {
            url: Some("https://b.com".into()),
            title: Some("B".into()),
            app_name: None,
            interactive_elements: None,
        });
        assert!(ScreenMonitor::context_differs(&a, &b));
        assert!(!ScreenMonitor::context_differs(&a, &a));
        assert!(!ScreenMonitor::context_differs(&None, &None));
        assert!(ScreenMonitor::context_differs(&None, &a));
        assert!(ScreenMonitor::context_differs(&a, &None));
    }
}
