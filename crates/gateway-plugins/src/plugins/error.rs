//! Plugin error types.

use thiserror::Error;

/// Errors that can occur in the plugin system.
#[derive(Debug, Error)]
pub enum PluginError {
    /// Plugin not found in catalog or filesystem.
    #[error("plugin not found: {0}")]
    NotFound(String),

    /// Plugin directory exists but has no recognized format markers.
    #[error("unknown plugin format at {path}: {hint}")]
    UnknownFormat {
        /// Path to the unrecognized directory.
        path: String,
        /// Hint about what was expected.
        hint: String,
    },

    /// Plugin manifest is malformed or missing required fields.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    /// Filesystem I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Parse error (YAML, JSON, or SKILL.md).
    #[error("parse error: {0}")]
    Parse(String),

    /// Plugin is already installed for the given user.
    #[error("plugin already installed: {0}")]
    AlreadyInstalled(String),

    /// Plugin is not installed for the given user.
    #[error("plugin not installed: {0}")]
    NotInstalled(String),

    /// Serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for PluginError {
    fn from(err: serde_json::Error) -> Self {
        PluginError::Serialization(err.to_string())
    }
}

/// Result type for plugin operations.
pub type PluginResult<T> = Result<T, PluginError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_error_display() {
        let err = PluginError::NotFound("pdf".to_string());
        assert_eq!(err.to_string(), "plugin not found: pdf");

        let err = PluginError::UnknownFormat {
            path: "/plugins/mystery".to_string(),
            hint: "Expected .claude-plugin/plugin.json or SKILL.md".to_string(),
        };
        assert!(err.to_string().contains("unknown plugin format"));
        assert!(err.to_string().contains("/plugins/mystery"));

        let err = PluginError::AlreadyInstalled("pdf".to_string());
        assert!(err.to_string().contains("already installed"));

        let err = PluginError::NotInstalled("pdf".to_string());
        assert!(err.to_string().contains("not installed"));
    }

    #[test]
    fn test_plugin_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let plugin_err: PluginError = io_err.into();
        assert!(matches!(plugin_err, PluginError::Io(_)));
        assert!(plugin_err.to_string().contains("file missing"));
    }
}
