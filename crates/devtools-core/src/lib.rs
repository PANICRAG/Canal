//! devtools-core — Standalone LLM observability engine for Canal
//!
//! Provides a Langfuse-style data model: Trace -> Span -> Generation -> Event.
//! Zero gateway-core dependencies — pure observability logic.
//!
//! # Architecture
//!
//! - `types` — Core data model (Trace, Observation, Session, Project)
//! - `traits` — Pluggable storage and event bus traits
//! - `store` — In-memory implementation (DashMap + broadcast)
//! - `service` — Facade composing storage + event bus
//! - `client` — HTTP ingest client (feature = "client")

pub mod config;
pub mod error;
pub mod filter;
pub mod service;
pub mod store;
pub mod traits;
pub mod types;

#[cfg(feature = "client")]
pub mod client;

pub use error::DevtoolsError;
pub use filter::{MetricsFilter, ObservationUpdate, TraceFilter, TraceUpdate};
pub use service::DevtoolsService;
pub use traits::{EventBus, TraceExporter, TraceStore};
pub use types::*;
