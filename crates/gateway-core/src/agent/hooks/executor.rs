//! Hook Executor - Executes hooks in order with timeout and error handling

use super::{HookCallback, HookOutput, RegisteredHook};
use crate::agent::types::{HookContext, HookEvent, HookResult};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};

/// Hook executor manages and runs hooks
#[allow(dead_code)]
pub struct HookExecutor {
    /// Registered hooks by event
    hooks: RwLock<HashMap<HookEvent, Vec<RegisteredHook>>>,
    /// Default timeout for hooks
    default_timeout_ms: u64,
    /// Whether to continue on hook errors
    continue_on_error: bool,
}

impl Default for HookExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl HookExecutor {
    /// Create a new hook executor
    pub fn new() -> Self {
        Self {
            hooks: RwLock::new(HashMap::new()),
            default_timeout_ms: 60000,
            continue_on_error: true,
        }
    }

    /// Create with custom settings
    pub fn with_settings(default_timeout_ms: u64, continue_on_error: bool) -> Self {
        Self {
            hooks: RwLock::new(HashMap::new()),
            default_timeout_ms,
            continue_on_error,
        }
    }

    /// Register a hook
    pub async fn register(&self, hook: RegisteredHook) {
        let mut hooks = self.hooks.write().await;
        for event in &hook.events {
            hooks.entry(*event).or_default().push(hook.clone());
        }

        // Sort by priority (descending)
        for hooks_list in hooks.values_mut() {
            hooks_list.sort_by(|a, b| b.priority.cmp(&a.priority));
        }
    }

    /// Register a simple callback for specific events
    pub async fn on<F>(&self, events: Vec<HookEvent>, callback: Arc<dyn HookCallback>) {
        let hook = RegisteredHook::new(callback, events);
        self.register(hook).await;
    }

    /// Unregister all hooks for an event
    pub async fn clear_event(&self, event: HookEvent) {
        let mut hooks = self.hooks.write().await;
        hooks.remove(&event);
    }

    /// Unregister all hooks
    pub async fn clear_all(&self) {
        let mut hooks = self.hooks.write().await;
        hooks.clear();
    }

    /// Execute all hooks for an event
    pub async fn execute(
        &self,
        event: HookEvent,
        data: serde_json::Value,
        context: &HookContext,
    ) -> Vec<HookOutput> {
        self.execute_with_filter(event, data, context, None).await
    }

    /// Execute hooks for an event with optional tool filter
    pub async fn execute_with_filter(
        &self,
        event: HookEvent,
        data: serde_json::Value,
        context: &HookContext,
        tool_name: Option<&str>,
    ) -> Vec<HookOutput> {
        // R1-H2: Clone hooks list and release read lock immediately to avoid
        // holding it during entire async execution chain (which can take seconds).
        let hooks_list = {
            let hooks = self.hooks.read().await;
            match hooks.get(&event) {
                Some(list) => list.clone(),
                None => return vec![],
            }
        };

        let mut outputs = Vec::new();
        let mut current_data = data;

        for hook in &hooks_list {
            // Skip disabled hooks
            if !hook.enabled {
                continue;
            }

            // Check tool filter
            if let (Some(filter), Some(tool)) = (&hook.tool_filter, tool_name) {
                if !Self::matches_filter(filter, tool) {
                    continue;
                }
            }

            // Execute with timeout
            let start = Instant::now();
            let timeout_duration = Duration::from_millis(hook.timeout_ms);

            let result = timeout(
                timeout_duration,
                hook.callback.on_event(event, current_data.clone(), context),
            )
            .await;

            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(hook_result) => {
                    // Check if hook modified the data
                    if let HookResult::Continue {
                        modified_data: Some(new_data),
                    } = &hook_result
                    {
                        current_data = new_data.clone();
                    }

                    let output =
                        HookOutput::success(hook.callback.name(), hook_result.clone(), duration_ms);
                    outputs.push(output);

                    // Check if we should stop
                    if hook_result.is_cancel() {
                        break;
                    }
                }
                Err(_timeout_err) => {
                    let output = HookOutput::error(
                        hook.callback.name(),
                        format!("Hook timed out after {}ms", hook.timeout_ms),
                        duration_ms,
                    );
                    outputs.push(output);

                    if !self.continue_on_error {
                        break;
                    }
                }
            }
        }

        outputs
    }

    /// Execute hooks and return aggregated result
    pub async fn execute_and_aggregate(
        &self,
        event: HookEvent,
        data: serde_json::Value,
        context: &HookContext,
        tool_name: Option<&str>,
    ) -> (HookResult, Option<serde_json::Value>) {
        let outputs = self
            .execute_with_filter(event, data.clone(), context, tool_name)
            .await;

        // Find the final result
        let mut final_result = HookResult::continue_();
        let mut modified_data = None;

        for output in outputs {
            match &output.result {
                HookResult::Cancel { .. } => {
                    return (output.result, None);
                }
                HookResult::Continue {
                    modified_data: Some(data),
                } => {
                    modified_data = Some(data.clone());
                }
                HookResult::Retry {
                    modified_data: Some(data),
                    ..
                } => {
                    modified_data = Some(data.clone());
                    final_result = output.result;
                }
                _ => {}
            }
        }

        (final_result, modified_data)
    }

    /// Check if a tool name matches a filter pattern
    fn matches_filter(filter: &str, tool_name: &str) -> bool {
        if filter == "*" {
            return true;
        }

        if filter.starts_with('*') && filter.ends_with('*') {
            let middle = &filter[1..filter.len() - 1];
            return tool_name.contains(middle);
        }

        if filter.starts_with('*') {
            let suffix = &filter[1..];
            return tool_name.ends_with(suffix);
        }

        if filter.ends_with('*') {
            let prefix = &filter[..filter.len() - 1];
            return tool_name.starts_with(prefix);
        }

        filter == tool_name
    }

    /// Get hook count for an event
    pub async fn hook_count(&self, event: HookEvent) -> usize {
        let hooks = self.hooks.read().await;
        hooks.get(&event).map(|v| v.len()).unwrap_or(0)
    }

    /// Get total hook count
    pub async fn total_hook_count(&self) -> usize {
        let hooks = self.hooks.read().await;
        hooks.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct CountingHook {
        name: String,
        count: Arc<RwLock<u32>>,
    }

    #[async_trait]
    impl HookCallback for CountingHook {
        async fn on_event(
            &self,
            _event: HookEvent,
            _data: serde_json::Value,
            _context: &HookContext,
        ) -> HookResult {
            let mut count = self.count.write().await;
            *count += 1;
            HookResult::continue_()
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn handles_event(&self, _event: HookEvent) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_executor_register_and_execute() {
        let executor = HookExecutor::new();
        let count = Arc::new(RwLock::new(0u32));

        let hook = CountingHook {
            name: "counter".to_string(),
            count: count.clone(),
        };

        executor
            .register(RegisteredHook::new(
                Arc::new(hook),
                vec![HookEvent::PreToolUse],
            ))
            .await;

        let context = HookContext::default();
        executor
            .execute(HookEvent::PreToolUse, serde_json::json!({}), &context)
            .await;

        assert_eq!(*count.read().await, 1);
    }

    #[tokio::test]
    async fn test_executor_filter_matching() {
        assert!(HookExecutor::matches_filter("*", "anything"));
        assert!(HookExecutor::matches_filter("Bash*", "BashTool"));
        assert!(HookExecutor::matches_filter("*Tool", "BashTool"));
        assert!(HookExecutor::matches_filter("*ash*", "BashTool"));
        assert!(HookExecutor::matches_filter("Exact", "Exact"));
        assert!(!HookExecutor::matches_filter("Exact", "NotExact"));
    }

    #[tokio::test]
    async fn test_executor_priority_order() {
        let executor = HookExecutor::new();
        let order = Arc::new(RwLock::new(Vec::new()));

        struct OrderHook {
            name: String,
            order: Arc<RwLock<Vec<String>>>,
        }

        #[async_trait]
        impl HookCallback for OrderHook {
            async fn on_event(
                &self,
                _event: HookEvent,
                _data: serde_json::Value,
                _context: &HookContext,
            ) -> HookResult {
                self.order.write().await.push(self.name.clone());
                HookResult::continue_()
            }

            fn name(&self) -> &str {
                &self.name
            }

            fn handles_event(&self, _event: HookEvent) -> bool {
                true
            }
        }

        // Register hooks with different priorities
        executor
            .register(
                RegisteredHook::new(
                    Arc::new(OrderHook {
                        name: "low".to_string(),
                        order: order.clone(),
                    }),
                    vec![HookEvent::PreToolUse],
                )
                .with_priority(1),
            )
            .await;

        executor
            .register(
                RegisteredHook::new(
                    Arc::new(OrderHook {
                        name: "high".to_string(),
                        order: order.clone(),
                    }),
                    vec![HookEvent::PreToolUse],
                )
                .with_priority(10),
            )
            .await;

        let context = HookContext::default();
        executor
            .execute(HookEvent::PreToolUse, serde_json::json!({}), &context)
            .await;

        let order_vec = order.read().await;
        assert_eq!(order_vec[0], "high");
        assert_eq!(order_vec[1], "low");
    }
}
