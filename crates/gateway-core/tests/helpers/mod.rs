//! Shared test infrastructure for Canal Agent integration tests
//!
//! Provides mock implementations, benchmark harness, and scenario builders
//! for testing the Agent conversation loop across all capability dimensions.

pub mod bench_harness;
pub mod mock_llm;
pub mod mock_tools;
pub mod scenario_builder;

// A28 test infrastructure
pub mod mock_auth;
pub mod mock_rte;
