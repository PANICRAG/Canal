//! Plugin store system — dual-format plugin loading, catalog, and management.
//!
//! Supports two plugin formats:
//! - **Claude Skills**: Simple format with `SKILL.md` + optional scripts/references
//! - **Cowork**: Complex format with `.claude-plugin/plugin.json` + `.mcp.json` + commands + skills
//!
//! ## Architecture
//!
//! ```text
//! PluginManager (coordinator)
//!     → PluginCatalog (global registry)
//!         → PluginLoader (format detection + parsing)
//!             → LoadedPlugin (skills + references + scripts + MCP)
//!     → SubscriptionStore (per-user persistence)
//! ```

pub mod catalog;
pub mod connector;
pub mod error;
pub mod loader;
pub mod manager;
pub mod manifest;
pub mod subscription;

pub use catalog::PluginCatalog;
pub use connector::ConnectorResolver;
pub use error::{PluginError, PluginResult};
pub use loader::{LoadedPlugin, PluginLoader};
pub use manager::PluginManager;
pub use manifest::{
    CatalogEntry, McpServerEntry, PluginApiResponse, PluginFormat, PluginManifest, PluginMcpConfig,
};
pub use subscription::SubscriptionStore;
