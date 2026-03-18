//! Service boundary traits for dual-mode deployment.
//!
//! Each trait defines a service boundary that can be implemented:
//! - **Locally**: in-process, zero overhead (monolith mode)
//! - **Remotely**: via gRPC, independent process (distributed mode)
//!
//! ```rust,ignore
//! // AppState doesn't care which implementation:
//! pub struct AppState {
//!     pub llm: Arc<dyn LlmService>,
//!     pub tools: Arc<dyn ToolService>,
//!     pub memory: Arc<dyn MemoryService>,
//! }
//! ```

pub mod error;
pub mod llm;
pub mod memory;
pub mod tools;
pub mod trace;

pub use error::ServiceError;
pub use llm::LlmService;
pub use memory::MemoryService;
pub use tools::ToolService;
pub use trace::TraceContext;
