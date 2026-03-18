//! Screen module — ScreenController-backed browser automation.
//!
//! Replaces the legacy `browser/` module (20 files, ~20K LOC) with a thin
//! adapter layer over `canal_cv::ScreenController`. All browser operations
//! go through coordinate-based screen interaction + CDP direct commands.
//!
//! ## Architecture
//!
//! ```text
//! Agent Tools (computer_screenshot, computer_click, ...)
//!     │
//!     └── ScreenController trait (canal-cv)
//!         │
//!         └── CdpScreenController (this module)
//!             └── ws://host:port/devtools/browser (CDP WebSocket)
//! ```

pub mod cdp_controller;
mod error;
pub mod tools;

pub use cdp_controller::{CdpConfig, CdpScreenController};
pub use tools::{register_screen_tools, SCREEN_TOOL_NAMES};
