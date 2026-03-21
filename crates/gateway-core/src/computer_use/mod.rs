//! Computer Use module — thin re-export layer over `canal-cv` crate.
//!
//! Core CV types, traits, and implementations live in `canal-cv`.
//! This module provides:
//! 1. Re-exports of all canal-cv public types (backward compat)
//! 2. `GatewayCoreLlmClient` adapter (bridges LlmProvider → CvLlmClient)
//! 3. `UiTarsProvider` + `UiTarsParser` (OpenRouter vision model client)
//! 4. `UiTarsDetector` adapter (wraps UiTarsProvider as VisionDetector)
//!
//! Note: `BrowserScreenController` has been removed. Use `crate::screen::CdpScreenController`
//! which implements `ScreenController` via CDP directly without the browser module.

// -- UI-TARS vision model client (moved from browser/) --
pub mod uitars_parser;
pub mod uitars_provider;

// -- Adapters that depend on gateway-core internals --
pub mod llm_adapter;
pub mod uitars_adapter;

// -- Re-export everything from canal-cv --
pub use canal_cv::*;

// -- Re-export adapter types --
pub use llm_adapter::GatewayCoreLlmClient;
pub use uitars_adapter::UiTarsDetector;
pub use uitars_parser::{ScrollDirection, UiTarsAction, UiTarsParser};
pub use uitars_provider::{UiTarsClickResult, UiTarsProvider, UiTarsProviderConfig};
