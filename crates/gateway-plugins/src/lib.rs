//! Plugins and connectors crate
//!
//! Provides plugin management, connector bundles, skill definitions,
//! and subscription tracking.
//!
//! Extracted from `gateway-core::plugins` and `gateway-core::connectors`
//! as a standalone crate for faster compilation and independent versioning.

pub mod connectors;
pub mod plugins;
pub mod skills;

// Re-export commonly used types
pub use connectors::{
    BundleManager, CategoryResolver, McpConnectionTracker, McpRefTracker, RuntimeRegistry,
};
pub use plugins::{
    PluginCatalog, PluginError, PluginFormat, PluginLoader, PluginManager, PluginManifest,
    SubscriptionStore,
};
pub use skills::{Skill, SkillParser};
