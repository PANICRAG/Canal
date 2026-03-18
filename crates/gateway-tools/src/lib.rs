//! Gateway Tools — Tool execution implementations for Canal.
//!
//! This crate contains all tool execution logic extracted from `gateway-core`:
//! - `filesystem` — Secure file system access (read, write, search)
//! - `executor` — Code execution (Python, Bash, Node.js, Go, Rust, Docker)
//!
//! Future phases will add: browser/, vm/, tools/, plugins/, connectors/

pub mod error;
pub mod executor;
pub mod filesystem;
#[cfg(unix)]
pub mod vm;

pub use error::{ServiceError, ServiceResult};
