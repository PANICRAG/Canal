//! Code Executor Module — facade re-exporting from gateway-tools.
//!
//! The implementation has been moved to `gateway-tools::executor`.
//! Only `vm_strategy` (which depends on `crate::vm`) remains here.

// Re-export everything from gateway-tools
pub use gateway_tools::executor::*;

// vm_strategy stays in gateway-core (depends on crate::vm)
#[cfg(unix)]
pub mod vm_strategy;

// Re-export Firecracker strategy
#[cfg(unix)]
pub use vm_strategy::FirecrackerExecutionStrategy;
