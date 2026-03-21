//! Async Job System
//!
//! Provides persistent background job execution for long-running agent tasks.
//! Jobs are decoupled from HTTP connections — clients submit a job, disconnect,
//! and reconnect later to check results or stream events via SSE.
//!
//! # Architecture
//!
//! - **JobStore**: PostgreSQL-backed persistence for job lifecycle
//! - **JobScheduler**: Background loop that claims queued jobs and executes them
//! - **JobNotifier**: Webhook notifications on job completion/failure
//!
//! # Feature Gate
//!
//! This module requires the `jobs` feature flag, which depends on `graph`.

pub mod config;
pub mod error;
pub mod hitl;
pub mod notification;
pub mod scheduler;
pub mod store;
pub mod types;

pub use config::JobsConfig;
pub use error::JobError;
pub use hitl::{request_human_input, HITLOutcome, HITLRequest, PendingHITLInputs};
pub use notification::{JobNotifier, WebhookNotifier};
pub use scheduler::JobScheduler;
pub use store::JobStore;
pub use types::*;
