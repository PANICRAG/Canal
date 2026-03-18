//! # canal-module
//!
//! Module trait and shared context for Canal's composable architecture.
//! Each module (engine, session, sandbox, billing, identity, admin) implements
//! `CanalModule` and registers handles for inter-module communication.

mod config;
pub mod health;

pub use config::{ModuleFlags, CanalConfig};
pub use health::{HealthStatus, ModuleHealth};

use axum::Router;
use dashmap::DashMap;
use std::any::Any;
use std::sync::Arc;

/// Every Canal module implements this trait.
#[async_trait::async_trait]
pub trait CanalModule: Send + Sync + 'static {
    /// Module name (used in logs, config, health).
    fn name(&self) -> &str;

    /// Returns this module's Axum routes.
    fn routes(&self) -> Router;

    /// Health check.
    async fn health(&self) -> ModuleHealth;

    /// Graceful shutdown.
    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Minimal shared state passed to module initialization.
///
/// Modules own their internal state — this only contains truly shared resources.
pub struct SharedContext {
    pub config: Arc<CanalConfig>,
    pub handles: ModuleHandleRegistry,
}

impl SharedContext {
    /// Create a new shared context.
    pub fn new(config: CanalConfig) -> Self {
        Self {
            config: Arc::new(config),
            handles: ModuleHandleRegistry::new(),
        }
    }
}

/// Runtime registry for inter-module communication.
///
/// Modules register typed handles that other modules can discover at runtime.
/// Uses `Arc<dyn Any + Send + Sync>` for type erasure with `downcast`.
pub struct ModuleHandleRegistry {
    handles: DashMap<&'static str, Arc<dyn Any + Send + Sync>>,
}

impl ModuleHandleRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            handles: DashMap::new(),
        }
    }

    /// Register a handle under a name.
    pub fn register<T: Send + Sync + 'static>(&self, name: &'static str, handle: Arc<T>) {
        self.handles
            .insert(name, handle as Arc<dyn Any + Send + Sync>);
    }

    /// Retrieve a typed handle by name. Returns None if not found or wrong type.
    pub fn get<T: Send + Sync + 'static>(&self, name: &'static str) -> Option<Arc<T>> {
        let entry = self.handles.get(name)?;
        entry.value().clone().downcast::<T>().ok()
    }

    /// Check if a handle is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.handles.contains_key(name)
    }

    /// List all registered handle names.
    pub fn names(&self) -> Vec<&'static str> {
        self.handles.iter().map(|e| *e.key()).collect()
    }
}

impl Default for ModuleHandleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_registry_register_get() {
        let registry = ModuleHandleRegistry::new();
        let value: Arc<String> = Arc::new("hello".to_string());
        registry.register("test", value);

        let retrieved: Option<Arc<String>> = registry.get("test");
        assert!(retrieved.is_some());
        assert_eq!(*retrieved.unwrap(), "hello");
    }

    #[test]
    fn test_handle_registry_missing() {
        let registry = ModuleHandleRegistry::new();
        let result: Option<Arc<String>> = registry.get("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_handle_registry_wrong_type() {
        let registry = ModuleHandleRegistry::new();
        let value: Arc<String> = Arc::new("hello".to_string());
        registry.register("test", value);

        // Try to get as wrong type
        let result: Option<Arc<u64>> = registry.get("test");
        assert!(result.is_none());
    }

    #[test]
    fn test_handle_registry_contains() {
        let registry = ModuleHandleRegistry::new();
        assert!(!registry.contains("test"));

        let value: Arc<String> = Arc::new("hello".to_string());
        registry.register("test", value);
        assert!(registry.contains("test"));
    }

    #[test]
    fn test_handle_registry_names() {
        let registry = ModuleHandleRegistry::new();
        registry.register("a", Arc::new(1u32));
        registry.register("b", Arc::new(2u32));

        let mut names = registry.names();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn test_module_flags_from_env_all() {
        let flags = ModuleFlags::from_env("all");
        assert!(flags.engine);
        assert!(flags.session);
        assert!(flags.sandbox);
        assert!(flags.billing);
        assert!(flags.identity);
        assert!(flags.admin);
        // "all" excludes engine-only shims
        assert!(!flags.auth_shim);
        assert!(!flags.usage_counter);
    }

    #[test]
    fn test_module_flags_from_env_partial() {
        let flags = ModuleFlags::from_env("engine,identity");
        assert!(flags.engine);
        assert!(!flags.session);
        assert!(!flags.sandbox);
        assert!(!flags.billing);
        assert!(flags.identity);
        assert!(!flags.admin);
    }

    #[test]
    fn test_module_flags_from_env_empty() {
        let flags = ModuleFlags::from_env("");
        assert!(!flags.engine);
        assert!(!flags.session);
        assert!(!flags.sandbox);
        assert!(!flags.billing);
        assert!(!flags.identity);
        assert!(!flags.admin);
    }

    #[test]
    fn test_module_flags_with_spaces() {
        let flags = ModuleFlags::from_env("engine , session , billing");
        assert!(flags.engine);
        assert!(flags.session);
        assert!(!flags.sandbox);
        assert!(flags.billing);
        assert!(!flags.identity);
        assert!(!flags.admin);
    }

    #[test]
    fn test_config_defaults() {
        let config = CanalConfig::default();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 4000);
        assert!(!config.modules.engine);
    }

    #[test]
    fn test_shared_context_creation() {
        let config = CanalConfig::default();
        let ctx = SharedContext::new(config);
        assert_eq!(ctx.config.port, 4000);
        assert!(ctx.handles.names().is_empty());
    }

    #[test]
    fn test_mutual_exclusion_identity_auth_shim() {
        let mut flags = ModuleFlags::default();
        flags.identity = true;
        flags.auth_shim = true;
        assert!(flags.validate().is_err());

        // Only identity — OK
        flags.auth_shim = false;
        assert!(flags.validate().is_ok());

        // Only auth_shim — OK
        flags.identity = false;
        flags.auth_shim = true;
        assert!(flags.validate().is_ok());
    }

    #[test]
    fn test_mutual_exclusion_billing_usage_counter() {
        let mut flags = ModuleFlags::default();
        flags.billing = true;
        flags.usage_counter = true;
        assert!(flags.validate().is_err());

        // Only billing — OK
        flags.usage_counter = false;
        assert!(flags.validate().is_ok());

        // Only usage_counter — OK
        flags.billing = false;
        flags.usage_counter = true;
        assert!(flags.validate().is_ok());
    }

    #[test]
    fn test_modules_all_excludes_shim() {
        let flags = ModuleFlags::all();
        assert!(!flags.auth_shim);
        assert!(!flags.usage_counter);
        assert!(flags.validate().is_ok());
    }

    #[test]
    fn test_profile_platform() {
        let flags = ModuleFlags::from_env("platform");
        assert!(!flags.engine);
        assert!(flags.session);
        assert!(flags.identity);
        assert!(flags.billing);
        assert!(flags.admin);
        assert!(flags.platform);
        assert!(!flags.auth_shim);
        assert!(!flags.usage_counter);
        assert!(flags.validate().is_ok());
    }

    #[test]
    fn test_profile_engine_full() {
        let flags = ModuleFlags::from_env("engine-full");
        assert!(flags.engine);
        assert!(flags.session);
        assert!(flags.sandbox);
        assert!(!flags.identity);
        assert!(!flags.billing);
        assert!(!flags.admin);
        assert!(!flags.platform);
        assert!(flags.auth_shim);
        assert!(flags.usage_counter);
        assert!(flags.validate().is_ok());
    }

    #[test]
    fn test_profile_engine_lite() {
        let flags = ModuleFlags::from_env("engine-lite");
        assert!(flags.engine);
        assert!(!flags.session);
        assert!(!flags.sandbox);
        assert!(!flags.identity);
        assert!(!flags.billing);
        assert!(flags.auth_shim);
        assert!(!flags.usage_counter);
        assert!(flags.validate().is_ok());
    }

    #[test]
    fn test_engine_profile_from_csv() {
        let flags = ModuleFlags::from_env("engine,auth_shim,usage_counter");
        assert!(flags.engine);
        assert!(flags.auth_shim);
        assert!(flags.usage_counter);
        assert!(!flags.identity);
        assert!(!flags.billing);
        assert!(flags.validate().is_ok());
    }
}
