//! Local service implementations for monolith mode.
//!
//! Each `Local*Service` wraps the concrete type and implements the
//! corresponding service trait from `gateway-service-traits`.
//! In distributed mode, these would be replaced with gRPC-backed
//! `Remote*Service` implementations.

pub mod local_llm;
pub mod local_memory;
pub mod local_tools;

pub use local_llm::LocalLlmService;
pub use local_memory::LocalMemoryService;
pub use local_tools::LocalToolService;
