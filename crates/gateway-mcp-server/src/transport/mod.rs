//! Transport layer for MCP server
//!
//! Supports STDIO (default) and HTTP transports.

pub mod http;
pub mod stdio;

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use async_trait::async_trait;

/// Transport abstraction for MCP server
#[async_trait]
pub trait Transport: Send + Sync {
    /// Start the transport loop, receiving requests and sending responses
    async fn run<H>(&self, handler: H) -> anyhow::Result<()>
    where
        H: RequestHandler + Send + Sync + 'static;
}

/// Handler for incoming JSON-RPC requests
#[async_trait]
pub trait RequestHandler: Send + Sync {
    /// Handle a single JSON-RPC request and return a response
    async fn handle(&self, request: JsonRpcRequest) -> JsonRpcResponse;
}
