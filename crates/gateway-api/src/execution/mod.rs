//! Execution strategies for gateway-api
//!
//! Contains execution strategy implementations that depend on both
//! gateway-core (ExecutionStrategy trait) and gateway-orchestrator
//! (ContainerOrchestrator, TaskWorkerClient).

pub mod k8s_strategy;

pub use k8s_strategy::K8sExecutionStrategy;
