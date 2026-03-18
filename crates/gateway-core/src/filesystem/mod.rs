//! Filesystem Service Module — facade re-exporting from gateway-tools.
//!
//! The implementation has been moved to `gateway-tools::filesystem`.
//! This module provides backward compatibility for existing consumers.

pub use gateway_tools::filesystem::*;
