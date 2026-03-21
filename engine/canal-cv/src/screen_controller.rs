//! ScreenController trait — the central abstraction for screen access.
//!
//! Every CV module operates through this trait. Implementations:
//! - `DesktopScreenController` (CV1) — macOS native APIs
//! - `BrowserScreenController` (CV0) — wraps existing BrowserRouter
//! - `NoopScreenController` (CV0) — fallback when no screen available
//! - \[future\] `RemoteScreenController` — VNC/RDP

use async_trait::async_trait;

use crate::types::{ComputerUseError, ContextInfo, Modifier, MouseButton, ScreenCapture};

/// Abstraction over any screen surface (browser tab, desktop, remote VM).
///
/// All CV modules receive `Arc<dyn ScreenController>` and never depend on
/// a concrete implementation directly.
#[async_trait]
pub trait ScreenController: Send + Sync {
    /// Capture current screen. Returns original resolution JPEG.
    async fn capture(&self) -> Result<ScreenCapture, ComputerUseError>;

    /// Click at display pixel coordinates.
    async fn click(&self, x: u32, y: u32, button: MouseButton) -> Result<(), ComputerUseError>;

    /// Type a text string.
    async fn type_text(&self, text: &str) -> Result<(), ComputerUseError>;

    /// Press a key with optional modifiers.
    async fn key_press(&self, key: &str, modifiers: &[Modifier]) -> Result<(), ComputerUseError>;

    /// Scroll at current position or specified coordinates.
    async fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<(), ComputerUseError>;

    /// Drag from one point to another (display pixels).
    async fn drag(
        &self,
        from_x: u32,
        from_y: u32,
        to_x: u32,
        to_y: u32,
    ) -> Result<(), ComputerUseError>;

    /// Get display dimensions (width, height) in display pixels.
    fn display_size(&self) -> (u32, u32);

    /// Get current screen context (title, app name, interactive elements).
    /// Returns None if context is unavailable.
    fn context_info(&self) -> Option<ContextInfo>;
}

/// Fallback controller when no screen surface is available.
///
/// All capture/input methods return `ComputerUseError::NotConnected`.
/// Used on Linux CI or when Screen Recording permission is not granted on macOS.
pub struct NoopScreenController {
    display_size: (u32, u32),
}

impl NoopScreenController {
    /// Create with default 1920x1080 display size.
    pub fn new() -> Self {
        Self {
            display_size: (1920, 1080),
        }
    }

    /// Create with custom display size.
    pub fn with_size(width: u32, height: u32) -> Self {
        Self {
            display_size: (width, height),
        }
    }
}

impl Default for NoopScreenController {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ScreenController for NoopScreenController {
    async fn capture(&self) -> Result<ScreenCapture, ComputerUseError> {
        Err(ComputerUseError::NotConnected)
    }

    async fn click(&self, _x: u32, _y: u32, _button: MouseButton) -> Result<(), ComputerUseError> {
        Err(ComputerUseError::NotConnected)
    }

    async fn type_text(&self, _text: &str) -> Result<(), ComputerUseError> {
        Err(ComputerUseError::NotConnected)
    }

    async fn key_press(&self, _key: &str, _modifiers: &[Modifier]) -> Result<(), ComputerUseError> {
        Err(ComputerUseError::NotConnected)
    }

    async fn scroll(&self, _dx: f64, _dy: f64) -> Result<(), ComputerUseError> {
        Err(ComputerUseError::NotConnected)
    }

    async fn drag(&self, _fx: u32, _fy: u32, _tx: u32, _ty: u32) -> Result<(), ComputerUseError> {
        Err(ComputerUseError::NotConnected)
    }

    fn display_size(&self) -> (u32, u32) {
        self.display_size
    }

    fn context_info(&self) -> Option<ContextInfo> {
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[tokio::test]
    async fn test_noop_capture_fails() {
        let ctrl = NoopScreenController::new();
        assert!(ctrl.capture().await.is_err());
    }

    #[tokio::test]
    async fn test_noop_click_fails() {
        let ctrl = NoopScreenController::new();
        assert!(ctrl.click(100, 200, MouseButton::Left).await.is_err());
    }

    #[test]
    fn test_noop_display_size_default() {
        let ctrl = NoopScreenController::new();
        assert_eq!(ctrl.display_size(), (1920, 1080));
    }

    #[test]
    fn test_noop_display_size_custom() {
        let ctrl = NoopScreenController::with_size(2560, 1440);
        assert_eq!(ctrl.display_size(), (2560, 1440));
    }

    #[test]
    fn test_noop_context_info_none() {
        let ctrl = NoopScreenController::new();
        assert!(ctrl.context_info().is_none());
    }
}
