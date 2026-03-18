//! Connector system — unified tool hierarchy with categories, caching, and runtime management.
//!
//! ## Architecture
//!
//! ```text
//! ConnectorManager (coordinator, renamed from PluginManager)
//!     → ConnectorCatalog (global registry of connectors)
//!         → ConnectorLoader (format detection + parsing)
//!             → LoadedConnector (skills + references + scripts + MCP)
//!     → ConnectorSubscriptions (per-user install state)
//!     → CategoryResolver (~~category → connector mapping)
//!
//! BundleManager (plugin bundles = connectors + skills + prompts)
//!     → BundleLoader (load bundle definitions)
//!     → RuntimeRegistry (per-user active bundles)
//!     → CacheManager (versioned cache + SHA-256 manifest)
//! ```

pub mod bundles;
pub mod cache;
pub mod categories;
pub mod mcp_connection_tracker;
pub mod mcp_tracker;
pub mod runtime;

// Re-export bundle types
pub use bundles::{BundleDefinition, BundleLoader, BundleManager, BundleResolution, McpServerDef};
pub use cache::{CacheManager, CachedEntry, FileManifest};
pub use categories::{CategoryDefinition, CategoryResolver, ConnectorCategory};
pub use mcp_connection_tracker::{McpConnectionStatus, McpConnectionTracker};
pub use mcp_tracker::McpRefTracker;
pub use runtime::RuntimeRegistry;

// The existing plugin module types are aliased as connector types
// for backward compatibility. The real types live in `plugins/` module.
// This module adds the new hierarchy layers on top.
