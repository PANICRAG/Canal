//! canal-rpc — Vsock RPC server for the Canal VM sandbox.
//!
//! Provides code execution, file operations, and auth for the Linux guest VM.

pub mod auth;
pub mod executor;
pub mod file_ops;
pub mod protocol;
