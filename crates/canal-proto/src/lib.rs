//! Centralized protobuf definitions for all Canal gRPC services.
//!
//! This crate compiles all `.proto` files once and re-exports the generated
//! modules. Other crates depend on `canal-proto` instead of running
//! `tonic_build` independently, saving ~120s per full rebuild.

/// Agent orchestration service (server + client stubs).
pub mod agent {
    #![allow(clippy::all)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("canal.agent");
}

/// Common types shared across services (TraceContext, ServiceError).
pub mod common {
    #![allow(clippy::all)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("canal.common");
}

/// LLM routing service (server + client stubs).
pub mod llm {
    #![allow(clippy::all)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("canal.llm");
}

/// Memory storage and retrieval service (server + client stubs).
pub mod memory {
    #![allow(clippy::all)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("canal.memory");
}

/// Stateless tool execution service (server + client stubs).
pub mod tools {
    #![allow(clippy::all)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("canal.tools");
}

/// Task worker service for isolated Kubernetes pods (server + client stubs).
pub mod worker {
    #![allow(clippy::all)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("canal.worker");
}
