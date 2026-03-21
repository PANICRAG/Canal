//! VNC Server Configuration and Management for Firecracker VMs
//!
//! This module provides types and utilities for managing VNC server
//! configuration and connections within Firecracker microVMs, enabling
//! browser visualization for debugging and monitoring.
//!
//! # Architecture
//!
//! ```text
//! +------------------+     +------------------+     +------------------+
//! |   Desktop/Web    |     |   Gateway API    |     |  Firecracker VM  |
//! |   noVNC Client   |<--->|   VNC Proxy      |<--->|   x11vnc Server  |
//! +------------------+     +------------------+     +------------------+
//!                                                          |
//!                                                          v
//!                                                   +------------------+
//!                                                   |   Xvfb Display   |
//!                                                   |   + Chromium     |
//!                                                   +------------------+
//! ```
//!
//! # Features
//!
//! - VNC server configuration with authentication
//! - WebSocket proxy for browser-based access
//! - SSL/TLS support for secure connections
//! - Viewport resize support
//! - Connection status monitoring

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// VNC server configuration for a Firecracker VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VncConfig {
    /// Whether VNC server is enabled.
    pub enabled: bool,

    /// VNC server port (default: 5900).
    pub port: u16,

    /// WebSocket proxy port for noVNC (default: 6080).
    pub websocket_port: u16,

    /// Optional VNC password for authentication.
    /// If None, server runs without password protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Enable SSL/TLS encryption for VNC connections.
    #[serde(default)]
    pub ssl_enabled: bool,

    /// Path to SSL certificate file (PEM format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_cert_path: Option<String>,

    /// Path to SSL private key file (PEM format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_key_path: Option<String>,

    /// Restrict access to localhost only.
    #[serde(default)]
    pub localhost_only: bool,

    /// JPEG quality for image compression (1-100).
    #[serde(default = "default_quality")]
    pub quality: u8,

    /// Compression level (0-9).
    #[serde(default = "default_compress_level")]
    pub compress_level: u8,
}

fn default_quality() -> u8 {
    80
}

fn default_compress_level() -> u8 {
    6
}

impl Default for VncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 5900,
            websocket_port: 6080,
            password: None,
            ssl_enabled: false,
            ssl_cert_path: None,
            ssl_key_path: None,
            localhost_only: false,
            quality: 80,
            compress_level: 6,
        }
    }
}

impl VncConfig {
    /// Create a new VNC configuration with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a VNC configuration with VNC enabled.
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }

    /// Enable or disable VNC.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the VNC port.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the WebSocket port for noVNC.
    pub fn with_websocket_port(mut self, port: u16) -> Self {
        self.websocket_port = port;
        self
    }

    /// Set the VNC password.
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Enable SSL/TLS with certificate and key paths.
    pub fn with_ssl(mut self, cert_path: impl Into<String>, key_path: impl Into<String>) -> Self {
        self.ssl_enabled = true;
        self.ssl_cert_path = Some(cert_path.into());
        self.ssl_key_path = Some(key_path.into());
        self
    }

    /// Restrict access to localhost only.
    pub fn with_localhost_only(mut self, localhost_only: bool) -> Self {
        self.localhost_only = localhost_only;
        self
    }

    /// Set image quality (1-100).
    pub fn with_quality(mut self, quality: u8) -> Self {
        self.quality = quality.clamp(1, 100);
        self
    }

    /// Set compression level (0-9).
    pub fn with_compress_level(mut self, level: u8) -> Self {
        self.compress_level = level.clamp(0, 9);
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), VncConfigError> {
        if self.port == 0 {
            return Err(VncConfigError::InvalidPort(self.port));
        }

        if self.websocket_port == 0 {
            return Err(VncConfigError::InvalidPort(self.websocket_port));
        }

        if self.port == self.websocket_port {
            return Err(VncConfigError::PortConflict {
                vnc_port: self.port,
                websocket_port: self.websocket_port,
            });
        }

        if self.ssl_enabled {
            if self.ssl_cert_path.is_none() {
                return Err(VncConfigError::MissingSslCertificate);
            }
            if self.ssl_key_path.is_none() {
                return Err(VncConfigError::MissingSslKey);
            }
        }

        Ok(())
    }
}

/// Information about an active VNC connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VncInfo {
    /// Direct VNC URL (vnc://host:port).
    pub url: String,

    /// WebSocket URL for noVNC (ws://host:port or wss://host:port).
    pub websocket_url: String,

    /// noVNC HTML viewer URL (http://host:port/vnc.html).
    pub novnc_url: String,

    /// Current viewport width.
    pub width: u32,

    /// Current viewport height.
    pub height: u32,

    /// Whether authentication is required.
    pub auth_required: bool,

    /// Whether SSL is enabled.
    pub ssl_enabled: bool,
}

impl VncInfo {
    /// Create a new VNC info instance.
    pub fn new(host: impl Into<String>, config: &VncConfig, width: u32, height: u32) -> Self {
        let host = host.into();
        let ws_protocol = if config.ssl_enabled { "wss" } else { "ws" };
        let http_protocol = if config.ssl_enabled { "https" } else { "http" };

        Self {
            url: format!("vnc://{}:{}", host, config.port),
            websocket_url: format!("{}://{}:{}", ws_protocol, host, config.websocket_port),
            novnc_url: format!(
                "{}://{}:{}/vnc.html",
                http_protocol, host, config.websocket_port
            ),
            width,
            height,
            auth_required: config.password.is_some(),
            ssl_enabled: config.ssl_enabled,
        }
    }

    /// Create VNC info for a VM with the given IP address.
    pub fn for_vm(ip: Ipv4Addr, config: &VncConfig, width: u32, height: u32) -> Self {
        Self::new(ip.to_string(), config, width, height)
    }
}

/// VNC server status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VncStatus {
    /// VNC server is stopped.
    Stopped,
    /// VNC server is starting.
    Starting,
    /// VNC server is running.
    Running,
    /// VNC server status is unknown.
    Unknown,
    /// VNC server encountered an error.
    Error,
}

impl Default for VncStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

impl std::fmt::Display for VncStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VncStatus::Stopped => write!(f, "stopped"),
            VncStatus::Starting => write!(f, "starting"),
            VncStatus::Running => write!(f, "running"),
            VncStatus::Unknown => write!(f, "unknown"),
            VncStatus::Error => write!(f, "error"),
        }
    }
}

/// Detailed VNC server state information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VncState {
    /// Current status.
    pub status: VncStatus,

    /// VNC server port.
    pub vnc_port: u16,

    /// WebSocket port.
    pub websocket_port: u16,

    /// VNC server process ID (if running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vnc_pid: Option<u32>,

    /// Websockify process ID (if running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websockify_pid: Option<u32>,

    /// Current viewport width.
    pub width: u32,

    /// Current viewport height.
    pub height: u32,

    /// Whether authentication is enabled.
    pub auth_enabled: bool,

    /// Whether SSL is enabled.
    pub ssl_enabled: bool,

    /// Last status update timestamp (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,

    /// Error message (if status is Error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Default for VncState {
    fn default() -> Self {
        Self {
            status: VncStatus::Unknown,
            vnc_port: 5900,
            websocket_port: 6080,
            vnc_pid: None,
            websockify_pid: None,
            width: 1920,
            height: 1080,
            auth_enabled: true,
            ssl_enabled: false,
            timestamp: None,
            error: None,
        }
    }
}

impl VncState {
    /// Check if VNC is currently running.
    pub fn is_running(&self) -> bool {
        self.status == VncStatus::Running
    }

    /// Check if VNC is available (running or starting).
    pub fn is_available(&self) -> bool {
        matches!(self.status, VncStatus::Running | VncStatus::Starting)
    }
}

/// VNC viewport resize request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VncResizeRequest {
    /// New width in pixels.
    pub width: u32,

    /// New height in pixels.
    pub height: u32,
}

impl VncResizeRequest {
    /// Create a new resize request.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Validate the resize request.
    pub fn validate(&self) -> Result<(), VncConfigError> {
        if self.width < 640 || self.width > 3840 {
            return Err(VncConfigError::InvalidDimension {
                name: "width".to_string(),
                value: self.width,
                min: 640,
                max: 3840,
            });
        }

        if self.height < 480 || self.height > 2160 {
            return Err(VncConfigError::InvalidDimension {
                name: "height".to_string(),
                value: self.height,
                min: 480,
                max: 2160,
            });
        }

        Ok(())
    }
}

/// VNC viewport resize response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VncResizeResponse {
    /// Whether the resize was successful.
    pub success: bool,

    /// New width after resize.
    pub width: u32,

    /// New height after resize.
    pub height: u32,

    /// Method used for resize (xrandr, browser_viewport).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,

    /// Additional notes about the resize operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,

    /// Error message if resize failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// VNC configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum VncConfigError {
    #[error("Invalid port: {0}")]
    InvalidPort(u16),

    #[error(
        "Port conflict: VNC port {vnc_port} and WebSocket port {websocket_port} must be different"
    )]
    PortConflict { vnc_port: u16, websocket_port: u16 },

    #[error("SSL is enabled but certificate path is not set")]
    MissingSslCertificate,

    #[error("SSL is enabled but key path is not set")]
    MissingSslKey,

    #[error("Invalid {name}: {value} (must be {min}-{max})")]
    InvalidDimension {
        name: String,
        value: u32,
        min: u32,
        max: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vnc_config_default() {
        let config = VncConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.port, 5900);
        assert_eq!(config.websocket_port, 6080);
        assert!(config.password.is_none());
        assert!(!config.ssl_enabled);
    }

    #[test]
    fn test_vnc_config_enabled() {
        let config = VncConfig::enabled();
        assert!(config.enabled);
        assert_eq!(config.port, 5900);
    }

    #[test]
    fn test_vnc_config_builder() {
        let config = VncConfig::new()
            .with_enabled(true)
            .with_port(5901)
            .with_websocket_port(6081)
            .with_password("secret")
            .with_quality(90)
            .with_compress_level(8);

        assert!(config.enabled);
        assert_eq!(config.port, 5901);
        assert_eq!(config.websocket_port, 6081);
        assert_eq!(config.password, Some("secret".to_string()));
        assert_eq!(config.quality, 90);
        assert_eq!(config.compress_level, 8);
    }

    #[test]
    fn test_vnc_config_with_ssl() {
        let config = VncConfig::new().with_ssl("/path/to/cert.pem", "/path/to/key.pem");

        assert!(config.ssl_enabled);
        assert_eq!(config.ssl_cert_path, Some("/path/to/cert.pem".to_string()));
        assert_eq!(config.ssl_key_path, Some("/path/to/key.pem".to_string()));
    }

    #[test]
    fn test_vnc_config_validate_success() {
        let config = VncConfig::enabled();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_vnc_config_validate_port_conflict() {
        let config = VncConfig::new().with_port(5900).with_websocket_port(5900);

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            VncConfigError::PortConflict { .. }
        ));
    }

    #[test]
    fn test_vnc_config_validate_ssl_missing_cert() {
        let config = VncConfig {
            ssl_enabled: true,
            ssl_cert_path: None,
            ssl_key_path: Some("/path/to/key.pem".to_string()),
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            VncConfigError::MissingSslCertificate
        ));
    }

    #[test]
    fn test_vnc_config_clamp_values() {
        let config = VncConfig::new()
            .with_quality(150) // Should be clamped to 100
            .with_compress_level(15); // Should be clamped to 9

        assert_eq!(config.quality, 100);
        assert_eq!(config.compress_level, 9);
    }

    #[test]
    fn test_vnc_info_new() {
        let config = VncConfig::enabled().with_password("test");
        let info = VncInfo::new("192.168.1.10", &config, 1920, 1080);

        assert_eq!(info.url, "vnc://192.168.1.10:5900");
        assert_eq!(info.websocket_url, "ws://192.168.1.10:6080");
        assert_eq!(info.novnc_url, "http://192.168.1.10:6080/vnc.html");
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert!(info.auth_required);
        assert!(!info.ssl_enabled);
    }

    #[test]
    fn test_vnc_info_with_ssl() {
        let config = VncConfig::enabled().with_ssl("/cert.pem", "/key.pem");
        let info = VncInfo::new("10.0.0.5", &config, 1280, 720);

        assert_eq!(info.url, "vnc://10.0.0.5:5900");
        assert_eq!(info.websocket_url, "wss://10.0.0.5:6080");
        assert_eq!(info.novnc_url, "https://10.0.0.5:6080/vnc.html");
        assert!(info.ssl_enabled);
    }

    #[test]
    fn test_vnc_info_for_vm() {
        let config = VncConfig::enabled();
        let ip = Ipv4Addr::new(172, 16, 0, 10);
        let info = VncInfo::for_vm(ip, &config, 1920, 1080);

        assert_eq!(info.url, "vnc://172.16.0.10:5900");
    }

    #[test]
    fn test_vnc_status_display() {
        assert_eq!(VncStatus::Stopped.to_string(), "stopped");
        assert_eq!(VncStatus::Starting.to_string(), "starting");
        assert_eq!(VncStatus::Running.to_string(), "running");
        assert_eq!(VncStatus::Unknown.to_string(), "unknown");
        assert_eq!(VncStatus::Error.to_string(), "error");
    }

    #[test]
    fn test_vnc_state_is_running() {
        let mut state = VncState::default();
        assert!(!state.is_running());

        state.status = VncStatus::Running;
        assert!(state.is_running());
    }

    #[test]
    fn test_vnc_state_is_available() {
        let mut state = VncState::default();
        assert!(!state.is_available());

        state.status = VncStatus::Starting;
        assert!(state.is_available());

        state.status = VncStatus::Running;
        assert!(state.is_available());

        state.status = VncStatus::Stopped;
        assert!(!state.is_available());
    }

    #[test]
    fn test_vnc_resize_request_validate() {
        let valid = VncResizeRequest::new(1920, 1080);
        assert!(valid.validate().is_ok());

        let too_small = VncResizeRequest::new(320, 240);
        assert!(too_small.validate().is_err());

        let too_large = VncResizeRequest::new(8000, 4000);
        assert!(too_large.validate().is_err());
    }

    #[test]
    fn test_vnc_config_serialization() {
        let config = VncConfig::enabled().with_port(5901).with_password("secret");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: VncConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(config.port, deserialized.port);
        assert_eq!(config.password, deserialized.password);
    }

    #[test]
    fn test_vnc_info_serialization() {
        let config = VncConfig::enabled();
        let info = VncInfo::new("localhost", &config, 1920, 1080);

        let json = serde_json::to_string(&info).unwrap();
        let deserialized: VncInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(info.url, deserialized.url);
        assert_eq!(info.width, deserialized.width);
        assert_eq!(info.height, deserialized.height);
    }
}
