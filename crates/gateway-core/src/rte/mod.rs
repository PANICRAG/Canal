//! Remote Tool Execution (RTE) Protocol.
//!
//! Enables native clients (Windows, macOS) to execute tools locally
//! on the user's machine instead of in the cloud. The protocol uses
//! SSE events for server→client communication and HTTP POST for
//! client→server tool results.
//!
//! # Protocol Flow
//!
//! 1. Client sends `client_capabilities` in StreamChatRequest
//! 2. Server responds with `session_start` SSE event (contains HMAC secret)
//! 3. When agent needs a tool the client supports:
//!    - Server sends `tool_execute_request` via SSE (HMAC-signed)
//!    - Agent loop pauses on a oneshot channel
//!    - Client executes tool locally
//!    - Client POSTs result to `/api/tools/result` (HMAC-signed)
//!    - Server verifies HMAC and resumes the agent loop
//! 4. If client times out, fallback strategy kicks in
//!
//! # Security
//!
//! All tool requests and results are signed with HMAC-SHA256 using a
//! per-session secret. The secret is transmitted in the initial
//! `session_start` event over the authenticated SSE connection.

pub mod delegate;
pub mod error;
pub mod rate_limiter;
pub mod signing;
pub mod types;

// Re-exports for convenience
pub use delegate::{
    build_tool_request, should_delegate, DelegationResult, PendingToolExecutions,
    RteDelegationContext,
};
pub use error::RteError;
pub use rate_limiter::{
    EndpointCategory, RateLimitResult, RateLimitTier, RateLimiter, RateLimiterConfig,
};
pub use signing::RteSigner;
pub use types::{
    ClientCapabilities, FallbackStrategy, RteSseEvent, ToolExecuteRequest, ToolExecuteResult,
    ToolFallbackConfig,
};
