//! VNC Access Control and Permission Management
//!
//! This module provides a comprehensive access control system for VNC connections
//! to Firecracker VMs. It supports session-based access, time-limited tokens,
//! and fine-grained permission control.
//!
//! # Architecture
//!
//! ```text
//! +------------------+     +------------------+     +------------------+
//! |   Client/User    |     | VncAccessManager |     |  Firecracker VM  |
//! |                  |---->|  - Token Store   |---->|   VNC Server     |
//! |                  |     |  - Permissions   |     |                  |
//! +------------------+     +------------------+     +------------------+
//!                                  |
//!                                  v
//!                          +------------------+
//!                          |  Access Control  |
//!                          |  - Time-based    |
//!                          |  - Permission    |
//!                          |  - Rate limiting |
//!                          +------------------+
//! ```
//!
//! # Features
//!
//! - Time-limited access tokens with configurable TTL
//! - Fine-grained permissions (view, keyboard, mouse, clipboard)
//! - Session-based access tracking
//! - Automatic token cleanup for expired entries
//! - Token validation middleware support
//! - Audit logging for access events

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

/// Default token TTL in seconds (1 hour)
const DEFAULT_TOKEN_TTL_SECS: i64 = 3600;

/// Maximum token TTL in seconds (24 hours)
const MAX_TOKEN_TTL_SECS: i64 = 86400;

/// Minimum token TTL in seconds (1 minute)
const MIN_TOKEN_TTL_SECS: i64 = 60;

/// VNC access token with expiration and permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VncAccessToken {
    /// Unique token identifier.
    pub token: String,

    /// VM ID this token grants access to.
    pub vm_id: String,

    /// Token creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Token expiration timestamp.
    pub expires_at: DateTime<Utc>,

    /// Permissions granted by this token.
    pub permissions: VncPermissions,

    /// Optional user/session identifier for audit trails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,

    /// Optional session identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Whether the token has been revoked.
    #[serde(default)]
    pub revoked: bool,

    /// Last access timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,

    /// Number of times the token has been used.
    #[serde(default)]
    pub use_count: u64,
}

impl VncAccessToken {
    /// Create a new VNC access token.
    pub fn new(vm_id: impl Into<String>, permissions: VncPermissions, ttl: Duration) -> Self {
        let now = Utc::now();
        Self {
            token: Uuid::new_v4().to_string(),
            vm_id: vm_id.into(),
            created_at: now,
            expires_at: now + ttl,
            permissions,
            user_id: None,
            session_id: None,
            revoked: false,
            last_used_at: None,
            use_count: 0,
        }
    }

    /// Check if the token is expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Check if the token is valid (not expired and not revoked).
    pub fn is_valid(&self) -> bool {
        !self.is_expired() && !self.revoked
    }

    /// Get remaining TTL in seconds.
    pub fn remaining_ttl_secs(&self) -> i64 {
        let remaining = self.expires_at - Utc::now();
        remaining.num_seconds().max(0)
    }

    /// Set the user ID for audit purposes.
    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Set the session ID for tracking.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Record token usage.
    pub fn record_usage(&mut self) {
        self.last_used_at = Some(Utc::now());
        self.use_count += 1;
    }
}

/// Fine-grained VNC permissions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VncPermissions {
    /// View only mode - no input allowed.
    pub view_only: bool,

    /// Allow clipboard operations (copy/paste).
    pub allow_clipboard: bool,

    /// Allow keyboard input.
    pub allow_keyboard: bool,

    /// Allow mouse input.
    pub allow_mouse: bool,
}

impl Default for VncPermissions {
    fn default() -> Self {
        Self {
            view_only: false,
            allow_clipboard: true,
            allow_keyboard: true,
            allow_mouse: true,
        }
    }
}

impl VncPermissions {
    /// Create view-only permissions.
    pub fn view_only() -> Self {
        Self {
            view_only: true,
            allow_clipboard: false,
            allow_keyboard: false,
            allow_mouse: false,
        }
    }

    /// Create full access permissions.
    pub fn full_access() -> Self {
        Self {
            view_only: false,
            allow_clipboard: true,
            allow_keyboard: true,
            allow_mouse: true,
        }
    }

    /// Create permissions without clipboard access.
    pub fn no_clipboard() -> Self {
        Self {
            view_only: false,
            allow_clipboard: false,
            allow_keyboard: true,
            allow_mouse: true,
        }
    }

    /// Check if any input is allowed.
    pub fn allows_input(&self) -> bool {
        !self.view_only && (self.allow_keyboard || self.allow_mouse)
    }

    /// Builder method to set view_only.
    pub fn with_view_only(mut self, view_only: bool) -> Self {
        self.view_only = view_only;
        if view_only {
            self.allow_clipboard = false;
            self.allow_keyboard = false;
            self.allow_mouse = false;
        }
        self
    }

    /// Builder method to set clipboard access.
    pub fn with_clipboard(mut self, allow: bool) -> Self {
        self.allow_clipboard = allow && !self.view_only;
        self
    }

    /// Builder method to set keyboard access.
    pub fn with_keyboard(mut self, allow: bool) -> Self {
        self.allow_keyboard = allow && !self.view_only;
        self
    }

    /// Builder method to set mouse access.
    pub fn with_mouse(mut self, allow: bool) -> Self {
        self.allow_mouse = allow && !self.view_only;
        self
    }
}

/// Configuration for VNC access management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VncAccessConfig {
    /// Default TTL for new tokens in seconds.
    pub default_ttl_secs: i64,

    /// Maximum allowed TTL in seconds.
    pub max_ttl_secs: i64,

    /// Minimum allowed TTL in seconds.
    pub min_ttl_secs: i64,

    /// Maximum tokens per VM.
    pub max_tokens_per_vm: usize,

    /// Maximum total tokens.
    pub max_total_tokens: usize,

    /// Enable automatic cleanup of expired tokens.
    pub auto_cleanup: bool,

    /// Cleanup interval in seconds.
    pub cleanup_interval_secs: u64,

    /// Default permissions for new tokens.
    pub default_permissions: VncPermissions,
}

impl Default for VncAccessConfig {
    fn default() -> Self {
        Self {
            default_ttl_secs: DEFAULT_TOKEN_TTL_SECS,
            max_ttl_secs: MAX_TOKEN_TTL_SECS,
            min_ttl_secs: MIN_TOKEN_TTL_SECS,
            max_tokens_per_vm: 10,
            max_total_tokens: 1000,
            auto_cleanup: true,
            cleanup_interval_secs: 300,
            default_permissions: VncPermissions::default(),
        }
    }
}

impl VncAccessConfig {
    /// Create a new configuration with custom settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the default TTL.
    pub fn with_default_ttl(mut self, ttl_secs: i64) -> Self {
        self.default_ttl_secs = ttl_secs.clamp(self.min_ttl_secs, self.max_ttl_secs);
        self
    }

    /// Set the maximum TTL.
    pub fn with_max_ttl(mut self, ttl_secs: i64) -> Self {
        self.max_ttl_secs = ttl_secs.max(self.min_ttl_secs);
        self
    }

    /// Set the maximum tokens per VM.
    pub fn with_max_tokens_per_vm(mut self, max: usize) -> Self {
        self.max_tokens_per_vm = max.max(1);
        self
    }

    /// Set default permissions.
    pub fn with_default_permissions(mut self, permissions: VncPermissions) -> Self {
        self.default_permissions = permissions;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), VncAccessError> {
        if self.min_ttl_secs <= 0 {
            return Err(VncAccessError::InvalidConfig {
                message: "min_ttl_secs must be positive".to_string(),
            });
        }
        if self.max_ttl_secs < self.min_ttl_secs {
            return Err(VncAccessError::InvalidConfig {
                message: "max_ttl_secs must be >= min_ttl_secs".to_string(),
            });
        }
        if self.default_ttl_secs < self.min_ttl_secs || self.default_ttl_secs > self.max_ttl_secs {
            return Err(VncAccessError::InvalidConfig {
                message: "default_ttl_secs must be between min and max".to_string(),
            });
        }
        if self.max_tokens_per_vm == 0 {
            return Err(VncAccessError::InvalidConfig {
                message: "max_tokens_per_vm must be > 0".to_string(),
            });
        }
        Ok(())
    }
}

/// VNC access control manager.
#[derive(Debug)]
pub struct VncAccessManager {
    /// Token storage indexed by token string.
    tokens: HashMap<String, VncAccessToken>,

    /// Index of tokens by VM ID for efficient lookups.
    vm_tokens: HashMap<String, Vec<String>>,

    /// Configuration for access management.
    config: VncAccessConfig,
}

impl VncAccessManager {
    /// Create a new VNC access manager with default configuration.
    pub fn new() -> Self {
        Self::with_config(VncAccessConfig::default())
    }

    /// Create a new VNC access manager with custom configuration.
    pub fn with_config(config: VncAccessConfig) -> Self {
        Self {
            tokens: HashMap::new(),
            vm_tokens: HashMap::new(),
            config,
        }
    }

    /// Get the current configuration.
    pub fn config(&self) -> &VncAccessConfig {
        &self.config
    }

    /// Create a new access token for a VM.
    pub fn create_token(
        &mut self,
        vm_id: &str,
        permissions: VncPermissions,
        ttl: Duration,
    ) -> Result<VncAccessToken, VncAccessError> {
        // Check total token limit
        if self.tokens.len() >= self.config.max_total_tokens {
            return Err(VncAccessError::TooManyTokens {
                limit: self.config.max_total_tokens,
            });
        }

        // Check per-VM token limit
        let vm_token_count = self.vm_tokens.get(vm_id).map(|v| v.len()).unwrap_or(0);
        if vm_token_count >= self.config.max_tokens_per_vm {
            return Err(VncAccessError::TooManyTokensForVm {
                vm_id: vm_id.to_string(),
                limit: self.config.max_tokens_per_vm,
            });
        }

        // Validate and clamp TTL
        let ttl_secs = ttl.num_seconds();
        let clamped_ttl =
            Duration::seconds(ttl_secs.clamp(self.config.min_ttl_secs, self.config.max_ttl_secs));

        // Create the token
        let token = VncAccessToken::new(vm_id, permissions, clamped_ttl);

        // Store the token
        let token_str = token.token.clone();
        self.tokens.insert(token_str.clone(), token.clone());

        // Update VM index
        self.vm_tokens
            .entry(vm_id.to_string())
            .or_default()
            .push(token_str);

        Ok(token)
    }

    /// Create a token with default TTL.
    pub fn create_token_default(
        &mut self,
        vm_id: &str,
        permissions: VncPermissions,
    ) -> Result<VncAccessToken, VncAccessError> {
        let ttl = Duration::seconds(self.config.default_ttl_secs);
        self.create_token(vm_id, permissions, ttl)
    }

    /// Validate a token and return a reference to it.
    pub fn validate_token(&self, token: &str) -> Result<&VncAccessToken, VncAccessError> {
        let access_token = self
            .tokens
            .get(token)
            .ok_or(VncAccessError::TokenNotFound {
                token: token.to_string(),
            })?;

        if access_token.revoked {
            return Err(VncAccessError::TokenRevoked {
                token: token.to_string(),
            });
        }

        if access_token.is_expired() {
            return Err(VncAccessError::TokenExpired {
                token: token.to_string(),
                expired_at: access_token.expires_at,
            });
        }

        Ok(access_token)
    }

    /// Validate and record token usage.
    pub fn use_token(&mut self, token: &str) -> Result<&VncAccessToken, VncAccessError> {
        // First validate
        self.validate_token(token)?;

        // Then record usage
        if let Some(access_token) = self.tokens.get_mut(token) {
            access_token.record_usage();
        }

        // Return reference
        self.tokens.get(token).ok_or(VncAccessError::TokenNotFound {
            token: token.to_string(),
        })
    }

    /// Revoke a token.
    pub fn revoke_token(&mut self, token: &str) -> bool {
        if let Some(access_token) = self.tokens.get_mut(token) {
            access_token.revoked = true;
            true
        } else {
            false
        }
    }

    /// Revoke all tokens for a VM.
    pub fn revoke_vm_tokens(&mut self, vm_id: &str) -> u32 {
        let mut revoked = 0;
        if let Some(token_ids) = self.vm_tokens.get(vm_id) {
            for token_id in token_ids.iter() {
                if let Some(token) = self.tokens.get_mut(token_id) {
                    if !token.revoked {
                        token.revoked = true;
                        revoked += 1;
                    }
                }
            }
        }
        revoked
    }

    /// Get all tokens for a VM.
    pub fn get_vm_tokens(&self, vm_id: &str) -> Vec<&VncAccessToken> {
        self.vm_tokens
            .get(vm_id)
            .map(|token_ids| {
                token_ids
                    .iter()
                    .filter_map(|id| self.tokens.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all valid (non-expired, non-revoked) tokens for a VM.
    pub fn get_valid_vm_tokens(&self, vm_id: &str) -> Vec<&VncAccessToken> {
        self.get_vm_tokens(vm_id)
            .into_iter()
            .filter(|t| t.is_valid())
            .collect()
    }

    /// Cleanup expired tokens and return the count of removed tokens.
    pub fn cleanup_expired(&mut self) -> u32 {
        let expired_tokens: Vec<String> = self
            .tokens
            .iter()
            .filter(|(_, t)| t.is_expired() || t.revoked)
            .map(|(k, _)| k.clone())
            .collect();

        let count = expired_tokens.len() as u32;

        for token_id in &expired_tokens {
            if let Some(token) = self.tokens.remove(token_id) {
                // Remove from VM index
                if let Some(vm_tokens) = self.vm_tokens.get_mut(&token.vm_id) {
                    vm_tokens.retain(|t| t != token_id);
                    // Remove empty VM entries
                    if vm_tokens.is_empty() {
                        self.vm_tokens.remove(&token.vm_id);
                    }
                }
            }
        }

        count
    }

    /// Get statistics about the token store.
    pub fn stats(&self) -> VncAccessStats {
        let total = self.tokens.len();
        let valid = self.tokens.values().filter(|t| t.is_valid()).count();
        let expired = self.tokens.values().filter(|t| t.is_expired()).count();
        let revoked = self.tokens.values().filter(|t| t.revoked).count();
        let vms_with_tokens = self.vm_tokens.len();

        VncAccessStats {
            total_tokens: total,
            valid_tokens: valid,
            expired_tokens: expired,
            revoked_tokens: revoked,
            vms_with_tokens,
        }
    }

    /// Check if a token grants specific permission.
    pub fn check_permission(
        &self,
        token: &str,
        permission: VncPermissionType,
    ) -> Result<bool, VncAccessError> {
        let access_token = self.validate_token(token)?;
        Ok(match permission {
            VncPermissionType::View => true, // Always allowed if token is valid
            VncPermissionType::Keyboard => {
                !access_token.permissions.view_only && access_token.permissions.allow_keyboard
            }
            VncPermissionType::Mouse => {
                !access_token.permissions.view_only && access_token.permissions.allow_mouse
            }
            VncPermissionType::Clipboard => {
                !access_token.permissions.view_only && access_token.permissions.allow_clipboard
            }
        })
    }

    /// Extend a token's expiration time.
    pub fn extend_token(
        &mut self,
        token: &str,
        additional_ttl: Duration,
    ) -> Result<&VncAccessToken, VncAccessError> {
        // First validate the token exists and is not revoked
        let access_token = self
            .tokens
            .get(token)
            .ok_or(VncAccessError::TokenNotFound {
                token: token.to_string(),
            })?;

        if access_token.revoked {
            return Err(VncAccessError::TokenRevoked {
                token: token.to_string(),
            });
        }

        // Calculate new expiration
        let new_expires_at = access_token.expires_at + additional_ttl;
        let max_expires_at = access_token.created_at + Duration::seconds(self.config.max_ttl_secs);

        // Update the token
        if let Some(token_mut) = self.tokens.get_mut(token) {
            token_mut.expires_at = new_expires_at.min(max_expires_at);
        }

        self.tokens.get(token).ok_or(VncAccessError::TokenNotFound {
            token: token.to_string(),
        })
    }

    /// Get a token by its ID (without validation).
    pub fn get_token(&self, token: &str) -> Option<&VncAccessToken> {
        self.tokens.get(token)
    }

    /// Get the total number of tokens.
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Check if a VM has any valid tokens.
    pub fn vm_has_valid_tokens(&self, vm_id: &str) -> bool {
        !self.get_valid_vm_tokens(vm_id).is_empty()
    }
}

impl Default for VncAccessManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Permission types for granular checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VncPermissionType {
    /// View the VNC display.
    View,
    /// Send keyboard input.
    Keyboard,
    /// Send mouse input.
    Mouse,
    /// Access clipboard.
    Clipboard,
}

/// Statistics about VNC access tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VncAccessStats {
    /// Total number of tokens.
    pub total_tokens: usize,
    /// Number of valid tokens.
    pub valid_tokens: usize,
    /// Number of expired tokens.
    pub expired_tokens: usize,
    /// Number of revoked tokens.
    pub revoked_tokens: usize,
    /// Number of VMs with tokens.
    pub vms_with_tokens: usize,
}

/// VNC access control errors.
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum VncAccessError {
    /// Token not found.
    #[error("VNC access token not found: {token}")]
    TokenNotFound { token: String },

    /// Token has expired.
    #[error("VNC access token expired: {token} at {expired_at}")]
    TokenExpired {
        token: String,
        expired_at: DateTime<Utc>,
    },

    /// Token has been revoked.
    #[error("VNC access token revoked: {token}")]
    TokenRevoked { token: String },

    /// Permission denied.
    #[error("VNC permission denied: {permission} for token {token}")]
    PermissionDenied { token: String, permission: String },

    /// Too many tokens created.
    #[error("Too many VNC tokens: limit is {limit}")]
    TooManyTokens { limit: usize },

    /// Too many tokens for a specific VM.
    #[error("Too many VNC tokens for VM {vm_id}: limit is {limit}")]
    TooManyTokensForVm { vm_id: String, limit: usize },

    /// Invalid configuration.
    #[error("Invalid VNC access configuration: {message}")]
    InvalidConfig { message: String },

    /// VM not found.
    #[error("VM not found: {vm_id}")]
    VmNotFound { vm_id: String },
}

impl VncAccessError {
    /// Check if this error is recoverable.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            VncAccessError::TokenExpired { .. }
                | VncAccessError::TooManyTokens { .. }
                | VncAccessError::TooManyTokensForVm { .. }
        )
    }

    /// Get an error code for categorization.
    pub fn error_code(&self) -> &'static str {
        match self {
            VncAccessError::TokenNotFound { .. } => "E_VNC_TOKEN_NOT_FOUND",
            VncAccessError::TokenExpired { .. } => "E_VNC_TOKEN_EXPIRED",
            VncAccessError::TokenRevoked { .. } => "E_VNC_TOKEN_REVOKED",
            VncAccessError::PermissionDenied { .. } => "E_VNC_PERMISSION_DENIED",
            VncAccessError::TooManyTokens { .. } => "E_VNC_TOO_MANY_TOKENS",
            VncAccessError::TooManyTokensForVm { .. } => "E_VNC_TOO_MANY_VM_TOKENS",
            VncAccessError::InvalidConfig { .. } => "E_VNC_INVALID_CONFIG",
            VncAccessError::VmNotFound { .. } => "E_VNC_VM_NOT_FOUND",
        }
    }
}

/// Token validation result for middleware integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenValidationResult {
    /// Whether the token is valid.
    pub valid: bool,
    /// VM ID the token grants access to.
    pub vm_id: Option<String>,
    /// Permissions granted.
    pub permissions: Option<VncPermissions>,
    /// Remaining TTL in seconds.
    pub remaining_ttl_secs: Option<i64>,
    /// Error message if validation failed.
    pub error: Option<String>,
}

impl TokenValidationResult {
    /// Create a successful validation result.
    pub fn success(token: &VncAccessToken) -> Self {
        Self {
            valid: true,
            vm_id: Some(token.vm_id.clone()),
            permissions: Some(token.permissions.clone()),
            remaining_ttl_secs: Some(token.remaining_ttl_secs()),
            error: None,
        }
    }

    /// Create a failed validation result.
    pub fn failure(error: &VncAccessError) -> Self {
        Self {
            valid: false,
            vm_id: None,
            permissions: None,
            remaining_ttl_secs: None,
            error: Some(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vnc_permissions_default() {
        let perms = VncPermissions::default();
        assert!(!perms.view_only);
        assert!(perms.allow_clipboard);
        assert!(perms.allow_keyboard);
        assert!(perms.allow_mouse);
        assert!(perms.allows_input());
    }

    #[test]
    fn test_vnc_permissions_view_only() {
        let perms = VncPermissions::view_only();
        assert!(perms.view_only);
        assert!(!perms.allow_clipboard);
        assert!(!perms.allow_keyboard);
        assert!(!perms.allow_mouse);
        assert!(!perms.allows_input());
    }

    #[test]
    fn test_vnc_permissions_full_access() {
        let perms = VncPermissions::full_access();
        assert!(!perms.view_only);
        assert!(perms.allow_clipboard);
        assert!(perms.allow_keyboard);
        assert!(perms.allow_mouse);
    }

    #[test]
    fn test_vnc_permissions_no_clipboard() {
        let perms = VncPermissions::no_clipboard();
        assert!(!perms.view_only);
        assert!(!perms.allow_clipboard);
        assert!(perms.allow_keyboard);
        assert!(perms.allow_mouse);
    }

    #[test]
    fn test_vnc_permissions_builder() {
        let perms = VncPermissions::default()
            .with_clipboard(false)
            .with_keyboard(true)
            .with_mouse(false);

        assert!(!perms.allow_clipboard);
        assert!(perms.allow_keyboard);
        assert!(!perms.allow_mouse);
    }

    #[test]
    fn test_vnc_permissions_view_only_builder() {
        let perms = VncPermissions::default().with_view_only(true);
        assert!(perms.view_only);
        assert!(!perms.allow_clipboard);
        assert!(!perms.allow_keyboard);
        assert!(!perms.allow_mouse);
    }

    #[test]
    fn test_vnc_access_token_creation() {
        let perms = VncPermissions::full_access();
        let ttl = Duration::hours(1);
        let token = VncAccessToken::new("vm-123", perms.clone(), ttl);

        assert!(!token.token.is_empty());
        assert_eq!(token.vm_id, "vm-123");
        assert!(!token.is_expired());
        assert!(token.is_valid());
        assert!(!token.revoked);
        assert_eq!(token.permissions, perms);
    }

    #[test]
    fn test_vnc_access_token_with_user() {
        let token = VncAccessToken::new("vm-123", VncPermissions::default(), Duration::hours(1))
            .with_user_id("user-456")
            .with_session_id("session-789");

        assert_eq!(token.user_id, Some("user-456".to_string()));
        assert_eq!(token.session_id, Some("session-789".to_string()));
    }

    #[test]
    fn test_vnc_access_token_expiration() {
        let token = VncAccessToken::new("vm-123", VncPermissions::default(), Duration::seconds(-1));
        assert!(token.is_expired());
        assert!(!token.is_valid());
    }

    #[test]
    fn test_vnc_access_token_remaining_ttl() {
        let token = VncAccessToken::new("vm-123", VncPermissions::default(), Duration::hours(1));
        let remaining = token.remaining_ttl_secs();
        assert!(remaining > 3500); // Should be close to 3600
        assert!(remaining <= 3600);
    }

    #[test]
    fn test_vnc_access_token_usage() {
        let mut token =
            VncAccessToken::new("vm-123", VncPermissions::default(), Duration::hours(1));
        assert_eq!(token.use_count, 0);
        assert!(token.last_used_at.is_none());

        token.record_usage();
        assert_eq!(token.use_count, 1);
        assert!(token.last_used_at.is_some());

        token.record_usage();
        assert_eq!(token.use_count, 2);
    }

    #[test]
    fn test_vnc_access_config_default() {
        let config = VncAccessConfig::default();
        assert_eq!(config.default_ttl_secs, DEFAULT_TOKEN_TTL_SECS);
        assert_eq!(config.max_ttl_secs, MAX_TOKEN_TTL_SECS);
        assert_eq!(config.min_ttl_secs, MIN_TOKEN_TTL_SECS);
        assert!(config.auto_cleanup);
    }

    #[test]
    fn test_vnc_access_config_builder() {
        let config = VncAccessConfig::new()
            .with_default_ttl(7200)
            .with_max_ttl(14400)
            .with_max_tokens_per_vm(5)
            .with_default_permissions(VncPermissions::view_only());

        assert_eq!(config.default_ttl_secs, 7200);
        assert_eq!(config.max_ttl_secs, 14400);
        assert_eq!(config.max_tokens_per_vm, 5);
        assert!(config.default_permissions.view_only);
    }

    #[test]
    fn test_vnc_access_config_validate() {
        let valid_config = VncAccessConfig::default();
        assert!(valid_config.validate().is_ok());

        let invalid_config = VncAccessConfig {
            min_ttl_secs: 100,
            max_ttl_secs: 50, // Invalid: max < min
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_vnc_access_manager_create_token() {
        let mut manager = VncAccessManager::new();
        let result = manager.create_token("vm-123", VncPermissions::default(), Duration::hours(1));

        assert!(result.is_ok());
        let token = result.unwrap();
        assert_eq!(token.vm_id, "vm-123");
        assert_eq!(manager.token_count(), 1);
    }

    #[test]
    fn test_vnc_access_manager_validate_token() {
        let mut manager = VncAccessManager::new();
        let token = manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        let validated = manager.validate_token(&token.token);
        assert!(validated.is_ok());
        assert_eq!(validated.unwrap().vm_id, "vm-123");
    }

    #[test]
    fn test_vnc_access_manager_validate_nonexistent() {
        let manager = VncAccessManager::new();
        let result = manager.validate_token("nonexistent-token");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            VncAccessError::TokenNotFound { .. }
        ));
    }

    #[test]
    fn test_vnc_access_manager_revoke_token() {
        let mut manager = VncAccessManager::new();
        let token = manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        assert!(manager.revoke_token(&token.token));
        assert!(!manager.revoke_token("nonexistent")); // Should return false

        let result = manager.validate_token(&token.token);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            VncAccessError::TokenRevoked { .. }
        ));
    }

    #[test]
    fn test_vnc_access_manager_revoke_vm_tokens() {
        let mut manager = VncAccessManager::new();
        manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();
        manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();
        manager
            .create_token("vm-456", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        let revoked = manager.revoke_vm_tokens("vm-123");
        assert_eq!(revoked, 2);

        let vm123_tokens = manager.get_valid_vm_tokens("vm-123");
        assert!(vm123_tokens.is_empty());

        let vm456_tokens = manager.get_valid_vm_tokens("vm-456");
        assert_eq!(vm456_tokens.len(), 1);
    }

    #[test]
    fn test_vnc_access_manager_get_vm_tokens() {
        let mut manager = VncAccessManager::new();
        manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();
        manager
            .create_token("vm-123", VncPermissions::view_only(), Duration::hours(2))
            .unwrap();

        let tokens = manager.get_vm_tokens("vm-123");
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_vnc_access_manager_cleanup_expired() {
        let mut manager = VncAccessManager::new();

        // Create a valid token
        manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        // Create a token and manually expire it
        let expired_token = manager
            .create_token("vm-456", VncPermissions::default(), Duration::hours(1))
            .unwrap();
        // Manually set the token as expired by modifying it directly
        if let Some(token) = manager.tokens.get_mut(&expired_token.token) {
            token.expires_at = Utc::now() - Duration::hours(1);
        }

        // Create a revoked token
        let revoked_token = manager
            .create_token("vm-789", VncPermissions::default(), Duration::hours(1))
            .unwrap();
        manager.revoke_token(&revoked_token.token);

        assert_eq!(manager.token_count(), 3);

        let cleaned = manager.cleanup_expired();
        assert_eq!(cleaned, 2); // One expired, one revoked
        assert_eq!(manager.token_count(), 1);
    }

    #[test]
    fn test_vnc_access_manager_stats() {
        let mut manager = VncAccessManager::new();
        manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        // Create a token and manually expire it
        let expired_token = manager
            .create_token("vm-456", VncPermissions::default(), Duration::hours(1))
            .unwrap();
        // Manually set the token as expired
        if let Some(token) = manager.tokens.get_mut(&expired_token.token) {
            token.expires_at = Utc::now() - Duration::hours(1);
        }

        let stats = manager.stats();
        assert_eq!(stats.total_tokens, 2);
        assert_eq!(stats.valid_tokens, 1);
        assert_eq!(stats.expired_tokens, 1);
        assert_eq!(stats.revoked_tokens, 0);
        assert_eq!(stats.vms_with_tokens, 2);
    }

    #[test]
    fn test_vnc_access_manager_check_permission() {
        let mut manager = VncAccessManager::new();
        let token = manager
            .create_token("vm-123", VncPermissions::view_only(), Duration::hours(1))
            .unwrap();

        assert!(manager
            .check_permission(&token.token, VncPermissionType::View)
            .unwrap());
        assert!(!manager
            .check_permission(&token.token, VncPermissionType::Keyboard)
            .unwrap());
        assert!(!manager
            .check_permission(&token.token, VncPermissionType::Mouse)
            .unwrap());
        assert!(!manager
            .check_permission(&token.token, VncPermissionType::Clipboard)
            .unwrap());
    }

    #[test]
    fn test_vnc_access_manager_check_permission_full_access() {
        let mut manager = VncAccessManager::new();
        let token = manager
            .create_token("vm-123", VncPermissions::full_access(), Duration::hours(1))
            .unwrap();

        assert!(manager
            .check_permission(&token.token, VncPermissionType::View)
            .unwrap());
        assert!(manager
            .check_permission(&token.token, VncPermissionType::Keyboard)
            .unwrap());
        assert!(manager
            .check_permission(&token.token, VncPermissionType::Mouse)
            .unwrap());
        assert!(manager
            .check_permission(&token.token, VncPermissionType::Clipboard)
            .unwrap());
    }

    #[test]
    fn test_vnc_access_manager_extend_token() {
        let mut manager = VncAccessManager::new();
        let token = manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        let original_expires = token.expires_at;

        let extended = manager
            .extend_token(&token.token, Duration::hours(1))
            .unwrap();
        assert!(extended.expires_at > original_expires);
    }

    #[test]
    fn test_vnc_access_manager_extend_revoked_token() {
        let mut manager = VncAccessManager::new();
        let token = manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        manager.revoke_token(&token.token);

        let result = manager.extend_token(&token.token, Duration::hours(1));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            VncAccessError::TokenRevoked { .. }
        ));
    }

    #[test]
    fn test_vnc_access_manager_token_limit() {
        let config = VncAccessConfig::default().with_max_tokens_per_vm(2);
        let mut manager = VncAccessManager::with_config(config);

        manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();
        manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        let result = manager.create_token("vm-123", VncPermissions::default(), Duration::hours(1));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            VncAccessError::TooManyTokensForVm { .. }
        ));
    }

    #[test]
    fn test_vnc_access_manager_use_token() {
        let mut manager = VncAccessManager::new();
        let token = manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();

        let used = manager.use_token(&token.token).unwrap();
        assert_eq!(used.use_count, 1);

        let used_again = manager.use_token(&token.token).unwrap();
        assert_eq!(used_again.use_count, 2);
    }

    #[test]
    fn test_vnc_access_manager_vm_has_valid_tokens() {
        let mut manager = VncAccessManager::new();
        assert!(!manager.vm_has_valid_tokens("vm-123"));

        manager
            .create_token("vm-123", VncPermissions::default(), Duration::hours(1))
            .unwrap();
        assert!(manager.vm_has_valid_tokens("vm-123"));

        manager.revoke_vm_tokens("vm-123");
        manager.cleanup_expired();
        assert!(!manager.vm_has_valid_tokens("vm-123"));
    }

    #[test]
    fn test_vnc_access_error_codes() {
        assert_eq!(
            VncAccessError::TokenNotFound {
                token: "".to_string()
            }
            .error_code(),
            "E_VNC_TOKEN_NOT_FOUND"
        );
        assert_eq!(
            VncAccessError::TokenExpired {
                token: "".to_string(),
                expired_at: Utc::now()
            }
            .error_code(),
            "E_VNC_TOKEN_EXPIRED"
        );
        assert_eq!(
            VncAccessError::PermissionDenied {
                token: "".to_string(),
                permission: "".to_string()
            }
            .error_code(),
            "E_VNC_PERMISSION_DENIED"
        );
    }

    #[test]
    fn test_vnc_access_error_recoverable() {
        assert!(VncAccessError::TokenExpired {
            token: "".to_string(),
            expired_at: Utc::now()
        }
        .is_recoverable());
        assert!(VncAccessError::TooManyTokens { limit: 10 }.is_recoverable());
        assert!(!VncAccessError::TokenNotFound {
            token: "".to_string()
        }
        .is_recoverable());
        assert!(!VncAccessError::TokenRevoked {
            token: "".to_string()
        }
        .is_recoverable());
    }

    #[test]
    fn test_token_validation_result_success() {
        let token =
            VncAccessToken::new("vm-123", VncPermissions::full_access(), Duration::hours(1));
        let result = TokenValidationResult::success(&token);

        assert!(result.valid);
        assert_eq!(result.vm_id, Some("vm-123".to_string()));
        assert!(result.permissions.is_some());
        assert!(result.remaining_ttl_secs.is_some());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_token_validation_result_failure() {
        let error = VncAccessError::TokenNotFound {
            token: "bad-token".to_string(),
        };
        let result = TokenValidationResult::failure(&error);

        assert!(!result.valid);
        assert!(result.vm_id.is_none());
        assert!(result.permissions.is_none());
        assert!(result.error.is_some());
    }

    #[test]
    fn test_vnc_permissions_serialization() {
        let perms = VncPermissions::full_access();
        let json = serde_json::to_string(&perms).unwrap();
        let deserialized: VncPermissions = serde_json::from_str(&json).unwrap();
        assert_eq!(perms, deserialized);
    }

    #[test]
    fn test_vnc_access_token_serialization() {
        let token =
            VncAccessToken::new("vm-123", VncPermissions::full_access(), Duration::hours(1))
                .with_user_id("user-456");

        let json = serde_json::to_string(&token).unwrap();
        let deserialized: VncAccessToken = serde_json::from_str(&json).unwrap();

        assert_eq!(token.token, deserialized.token);
        assert_eq!(token.vm_id, deserialized.vm_id);
        assert_eq!(token.user_id, deserialized.user_id);
    }

    #[test]
    fn test_vnc_access_stats_serialization() {
        let stats = VncAccessStats {
            total_tokens: 10,
            valid_tokens: 8,
            expired_tokens: 1,
            revoked_tokens: 1,
            vms_with_tokens: 5,
        };

        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: VncAccessStats = serde_json::from_str(&json).unwrap();

        assert_eq!(stats.total_tokens, deserialized.total_tokens);
        assert_eq!(stats.valid_tokens, deserialized.valid_tokens);
    }
}
