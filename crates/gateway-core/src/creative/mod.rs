//! Creative Tool Abstraction Layer
//!
//! Provides a unified API for interacting with professional creative tools
//! including video editors, color grading software, and audio tools.
//!
//! # Supported Applications
//!
//! - **DaVinci Resolve**: Color grading, editing, Fairlight audio
//! - **Adobe Premiere Pro**: Video editing, Lumetri color
//! - **Final Cut Pro**: Video editing, color board
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │               Unified Creative API                       │
//! │  - timeline.add_clip(source, position, duration)        │
//! │  - color.apply_lut(clip, lut_file)                      │
//! │  - audio.normalize(clip, target_db)                     │
//! │  - export.render(timeline, format, settings)            │
//! └─────────────────────────────────────────────────────────┘
//!                          │
//!          ┌───────────────┼───────────────┐
//!          ▼               ▼               ▼
//! ┌────────────┐  ┌────────────┐  ┌────────────┐
//! │  DaVinci   │  │  Premiere  │  │ Final Cut  │
//! │  Adapter   │  │  Adapter   │  │  Adapter   │
//! └────────────┘  └────────────┘  └────────────┘
//! ```
//!
//! # Example Usage
//!
//! ```rust,ignore
//! use gateway_core::creative::{CreativeToolManager, Application};
//!
//! let manager = CreativeToolManager::new();
//!
//! // Auto-detect running application
//! let app = manager.detect_active_application().await?;
//!
//! // Use unified API
//! let clips = app.timeline().get_clips_by_tag("interview").await?;
//! for clip in clips {
//!     app.color().apply_lut(&clip, "/path/to/lut.cube").await?;
//!     app.audio().normalize(&clip, -16.0).await?;
//! }
//! ```

pub mod adapter;
pub mod api;
pub mod davinci;
pub mod premiere;
pub mod finalcut;
pub mod types;

pub use adapter::{CreativeAdapter, AdapterError, AdapterCapabilities};
pub use api::{CreativeToolManager, UnifiedApi};
pub use types::*;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Supported creative applications
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Application {
    DaVinciResolve,
    AdobePremiere,
    FinalCutPro,
    AfterEffects,
    Audition,
}

impl Application {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::DaVinciResolve => "DaVinci Resolve",
            Self::AdobePremiere => "Adobe Premiere Pro",
            Self::FinalCutPro => "Final Cut Pro",
            Self::AfterEffects => "Adobe After Effects",
            Self::Audition => "Adobe Audition",
        }
    }

    pub fn process_names(&self) -> &[&'static str] {
        match self {
            Self::DaVinciResolve => &["resolve", "Resolve"],
            Self::AdobePremiere => &["Adobe Premiere Pro", "premiere"],
            Self::FinalCutPro => &["Final Cut Pro", "FinalCutPro"],
            Self::AfterEffects => &["After Effects", "AfterFX"],
            Self::Audition => &["Adobe Audition", "Audition"],
        }
    }
}

/// Tool category for execution strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Safe, idempotent operations - can run in parallel
    ReadOnly,
    /// State-changing but reversible operations
    Reversible,
    /// Destructive or expensive operations - serial only
    Sensitive,
    /// External service calls - rate limited
    External,
}

/// Creative tool operation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationResult {
    pub success: bool,
    pub operation: String,
    pub target: Option<String>,
    pub details: HashMap<String, serde_json::Value>,
    pub duration_ms: u64,
    pub warnings: Vec<String>,
}

impl OperationResult {
    pub fn success(operation: impl Into<String>) -> Self {
        Self {
            success: true,
            operation: operation.into(),
            target: None,
            details: HashMap::new(),
            duration_ms: 0,
            warnings: vec![],
        }
    }

    pub fn failure(operation: impl Into<String>, error: impl Into<String>) -> Self {
        let mut details = HashMap::new();
        details.insert("error".to_string(), serde_json::Value::String(error.into()));
        Self {
            success: false,
            operation: operation.into(),
            target: None,
            details,
            duration_ms: 0,
            warnings: vec![],
        }
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    pub fn with_detail(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.details.insert(key.into(), value);
        self
    }

    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}
