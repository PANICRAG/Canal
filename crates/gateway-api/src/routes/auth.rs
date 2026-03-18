//! Authentication routes
//!
//! Provides user registration, login, token management, password reset,
//! session management, and TOTP two-factor authentication endpoints.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Extension, Json, Router,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Row};
use std::sync::Arc;
use totp_rs::{Algorithm, Secret, TOTP};
use uuid::Uuid;

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};
use canal_auth::KeyPair;


/// JWT token expiration in hours
const TOKEN_EXPIRATION_HOURS: i64 = 24;

/// Create the auth routes
pub fn routes() -> Router<AppState> {
    Router::new()
        // Public routes (no auth required)
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/forgot-password", post(forgot_password))
        .route("/reset-password", post(reset_password))
        // Authenticated routes
        .route("/me", get(get_current_user))
        .route("/refresh", post(refresh_token))
        .route("/change-password", post(change_password))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", delete(revoke_session))
        .route("/sessions/revoke-others", post(revoke_other_sessions))
        .route("/login-history", get(login_history))
        .route("/totp/setup", post(totp_setup))
        .route("/totp/verify-setup", post(totp_verify_setup))
        .route("/totp/disable", post(totp_disable))
}

// ---------------------------------------------------------------------------
// Helper Functions
// ---------------------------------------------------------------------------

/// Validate password strength: min 8 chars, must have uppercase, lowercase, digit
fn validate_password_strength(password: &str) -> Result<(), &'static str> {
    if password.len() < 8 {
        return Err("Password must be at least 8 characters");
    }
    if !password.chars().any(|c| c.is_uppercase()) {
        return Err("Password must contain at least one uppercase letter");
    }
    if !password.chars().any(|c| c.is_lowercase()) {
        return Err("Password must contain at least one lowercase letter");
    }
    if !password.chars().any(|c| c.is_ascii_digit()) {
        return Err("Password must contain at least one digit");
    }
    Ok(())
}

/// Generate a random token using UUID v4 hex (no dashes)
fn generate_random_token() -> String {
    Uuid::new_v4().as_simple().to_string()
}

/// Hash a token using SHA-256, returning hex string
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Extract client IP from request headers (X-Forwarded-For → X-Real-IP → None)
fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    // X-Forwarded-For may contain multiple IPs: "client, proxy1, proxy2"
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first_ip) = xff.split(',').next() {
            let ip = first_ip.trim();
            if !ip.is_empty() {
                return Some(ip.to_string());
            }
        }
    }
    if let Some(xri) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let ip = xri.trim();
        if !ip.is_empty() {
            return Some(ip.to_string());
        }
    }
    None
}

/// Extract User-Agent from request headers
fn extract_user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Build a TOTP instance from a Base32-encoded secret
fn build_totp(secret_base32: &str, email: &str) -> Result<TOTP, ApiError> {
    let secret = Secret::Encoded(secret_base32.to_string());
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret.to_bytes().map_err(|e| {
            ApiError::internal(format!("Invalid TOTP secret: {}", e))
        })?,
        Some("Canal".to_string()),
        email.to_string(),
    )
    .map_err(|e| ApiError::internal(format!("Failed to create TOTP: {}", e)))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// User registration request
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub name: Option<String>,
}

/// Login request
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub totp_code: Option<String>,
    #[serde(default)]
    pub recovery_code: Option<String>,
}

/// Auth response with token
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub user: UserResponse,
}

/// User response (without sensitive data)
#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    pub created_at: chrono::DateTime<Utc>,
}

/// Database user record
#[derive(Debug, FromRow)]
struct DbUser {
    id: Uuid,
    email: String,
    name: Option<String>,
    password_hash: Option<String>,
    role: Option<String>,
    status: Option<String>,
    created_at: chrono::DateTime<Utc>,
}

/// Forgot password request
#[derive(Debug, Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

/// Forgot password response
#[derive(Debug, Serialize)]
pub struct ForgotPasswordResponse {
    pub message: String,
    /// Reset token returned in dev mode; in production this would be sent via email
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_token: Option<String>,
}

/// Reset password request
#[derive(Debug, Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

/// Change password request (authenticated)
#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Session record for listing active sessions
#[derive(Debug, Serialize)]
pub struct SessionRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
    pub is_current: bool,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub last_active_at: Option<chrono::DateTime<Utc>>,
}

/// Login history entry
#[derive(Debug, Serialize)]
pub struct LoginHistoryEntry {
    pub id: Uuid,
    pub user_id: Uuid,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub login_at: chrono::DateTime<Utc>,
    pub success: bool,
}

/// TOTP setup response
#[derive(Debug, Serialize)]
pub struct TotpSetupResponse {
    pub secret: String,
    pub qr_code_base64: String,
    pub provisioning_uri: String,
}

/// TOTP verify setup request
#[derive(Debug, Deserialize)]
pub struct TotpVerifySetupRequest {
    pub secret: String,
    pub code: String,
}

/// TOTP verify setup response
#[derive(Debug, Serialize)]
pub struct TotpVerifySetupResponse {
    pub recovery_codes: Vec<String>,
}

/// TOTP disable request
#[derive(Debug, Deserialize)]
pub struct TotpDisableRequest {
    pub password: String,
}

/// Login history query parameters
#[derive(Debug, Deserialize)]
pub struct LoginHistoryQuery {
    #[serde(default = "default_history_limit")]
    pub limit: i64,
}

fn default_history_limit() -> i64 {
    50
}

// ---------------------------------------------------------------------------
// Existing Endpoints
// ---------------------------------------------------------------------------

/// Register a new user
pub async fn register(
    State(state): State<AppState>,
    Extension(key_pair): Extension<Arc<KeyPair>>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    // Validate email format
    if !req.email.contains('@') || req.email.len() < 5 {
        return Err(ApiError::bad_request("Invalid email format"));
    }

    // Validate password strength
    if let Err(msg) = validate_password_strength(&req.password) {
        return Err(ApiError::bad_request(msg));
    }

    // Check if email already exists
    let existing: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(&req.email)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    if existing.is_some() {
        // R4-M98: Generic message to prevent email enumeration attacks
        return Err(ApiError::bad_request("Registration failed. Please try again or use a different email."));
    }

    // Hash password (CPU-intensive — run off tokio executor)
    let password_to_hash = req.password.clone();
    let password_hash = tokio::task::spawn_blocking(move || {
        bcrypt::hash(&password_to_hash, bcrypt::DEFAULT_COST)
    })
    .await
    .map_err(|e| ApiError::internal(format!("Task join error: {}", e)))?
    .map_err(|e| ApiError::internal(format!("Password hashing error: {}", e)))?;

    // Create user
    let user: DbUser = sqlx::query_as(
        r#"
        INSERT INTO users (email, name, password_hash, role, status)
        VALUES ($1, $2, $3, 'user', 'active')
        RETURNING id, email, name, password_hash, role, status, created_at
        "#,
    )
    .bind(&req.email)
    .bind(&req.name)
    .bind(&password_hash)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Failed to create user: {}", e)))?;

    tracing::info!(user_id = %user.id, email = %user.email, "New user registered");

    // Generate RS256 JWT token
    let (token, expires_in) = generate_token(&user, &key_pair)?;

    Ok(Json(AuthResponse {
        token,
        token_type: "Bearer".to_string(),
        expires_in,
        user: UserResponse {
            id: user.id,
            email: user.email,
            name: user.name,
            role: user.role.unwrap_or_else(|| "user".to_string()),
            created_at: user.created_at,
        },
    }))
}

/// Response returned when login requires TOTP verification
#[derive(Debug, Serialize)]
struct TotpRequiredResponse {
    requires_totp: bool,
    message: String,
}

/// Login with email and password
pub async fn login(
    State(state): State<AppState>,
    Extension(key_pair): Extension<Arc<KeyPair>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<axum::response::Response, ApiError> {
    let client_ip = extract_client_ip(&headers);
    let user_agent = extract_user_agent(&headers);

    // Find user by email
    let user: Option<DbUser> = sqlx::query_as(
        r#"
        SELECT id, email, name, password_hash, role, status, created_at
        FROM users WHERE email = $1
        "#,
    )
    .bind(&req.email)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let user =
        user.ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "Invalid email or password"))?;

    // Check if user is active
    if user.status.as_deref() != Some("active") {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Account is not active",
        ));
    }

    // Verify password
    let password_hash = user
        .password_hash
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "Invalid email or password"))?;

    let pw_to_verify = req.password.clone();
    let hash_to_verify = password_hash.clone();
    let password_valid = tokio::task::spawn_blocking(move || {
        bcrypt::verify(&pw_to_verify, &hash_to_verify)
    })
    .await
    .map_err(|e| ApiError::internal(format!("Task join error: {}", e)))?
    .map_err(|e| ApiError::internal(format!("Password verification error: {}", e)))?;

    if !password_valid {
        // Record failed login attempt with IP/UA
        let _ = sqlx::query(
            "INSERT INTO login_history (user_id, ip_address, user_agent, login_method, success, failure_reason) VALUES ($1, $2, $3, 'password', false, 'invalid_password')",
        )
        .bind(user.id)
        .bind(&client_ip)
        .bind(&user_agent)
        .execute(&state.db)
        .await;

        tracing::warn!(email = %req.email, "Failed login attempt - invalid password");
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid email or password",
        ));
    }

    // TOTP two-factor authentication gate
    let totp_row = sqlx::query(
        "SELECT totp_enabled, totp_secret, recovery_codes FROM users WHERE id = $1",
    )
    .bind(user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    if let Some(row) = &totp_row {
        let totp_enabled: bool = row.try_get("totp_enabled").unwrap_or(false);
        if totp_enabled {
            let totp_secret_encrypted: Option<String> = row.try_get("totp_secret").ok().flatten();

            if let Some(ref code) = req.totp_code {
                // R4-C10: Decrypt TOTP secret from encrypted storage
                let secret_encrypted = totp_secret_encrypted.as_deref().ok_or_else(|| {
                    ApiError::internal("TOTP enabled but no secret stored")
                })?;
                let secret = crate::crypto::decrypt_credential(secret_encrypted)
                    .map_err(|e| ApiError::internal(format!("Failed to decrypt TOTP secret: {}", e)))?;
                let totp = build_totp(&secret, &user.email)?;
                if !totp.check_current(code).map_err(|e| {
                    ApiError::internal(format!("TOTP check error: {}", e))
                })? {
                    // Record failed TOTP attempt
                    let _ = sqlx::query(
                        "INSERT INTO login_history (user_id, ip_address, user_agent, login_method, success, failure_reason) VALUES ($1, $2, $3, 'totp', false, 'invalid_totp_code')",
                    )
                    .bind(user.id)
                    .bind(&client_ip)
                    .bind(&user_agent)
                    .execute(&state.db)
                    .await;

                    return Err(ApiError::new(
                        StatusCode::UNAUTHORIZED,
                        "Invalid TOTP code",
                    ));
                }
            } else if let Some(ref recovery) = req.recovery_code {
                // Verify recovery code
                let recovery_hash = hash_token(recovery);
                let stored_codes: Option<serde_json::Value> =
                    row.try_get("recovery_codes").ok().flatten();
                let codes = stored_codes
                    .as_ref()
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| {
                        ApiError::internal("No recovery codes found")
                    })?;

                let code_idx = codes.iter().position(|c| {
                    c.as_str().map(|s| s == recovery_hash).unwrap_or(false)
                });

                if let Some(idx) = code_idx {
                    // Consume the recovery code (remove from JSONB array)
                    let mut remaining: Vec<serde_json::Value> = codes.clone();
                    remaining.remove(idx);
                    let updated_json = serde_json::Value::Array(remaining);
                    let _ = sqlx::query(
                        "UPDATE users SET recovery_codes = $1 WHERE id = $2",
                    )
                    .bind(&updated_json)
                    .bind(user.id)
                    .execute(&state.db)
                    .await;
                } else {
                    // Record failed recovery code attempt
                    let _ = sqlx::query(
                        "INSERT INTO login_history (user_id, ip_address, user_agent, login_method, success, failure_reason) VALUES ($1, $2, $3, 'recovery', false, 'invalid_recovery_code')",
                    )
                    .bind(user.id)
                    .bind(&client_ip)
                    .bind(&user_agent)
                    .execute(&state.db)
                    .await;

                    return Err(ApiError::new(
                        StatusCode::UNAUTHORIZED,
                        "Invalid recovery code",
                    ));
                }
            } else {
                // No TOTP code or recovery code provided — return 403
                return Ok((
                    StatusCode::FORBIDDEN,
                    Json(TotpRequiredResponse {
                        requires_totp: true,
                        message: "Two-factor authentication required".to_string(),
                    }),
                )
                    .into_response());
            }
        }
    }

    // Update last login
    let _ = sqlx::query(
        "UPDATE users SET last_login_at = NOW(), login_count = COALESCE(login_count, 0) + 1 WHERE id = $1"
    )
    .bind(user.id)
    .execute(&state.db)
    .await;

    // Record login history with IP/UA
    let _ = sqlx::query(
        r#"
        INSERT INTO login_history (user_id, ip_address, user_agent, login_method, success)
        VALUES ($1, $2, $3, 'password', true)
        "#,
    )
    .bind(user.id)
    .bind(&client_ip)
    .bind(&user_agent)
    .execute(&state.db)
    .await;

    // Create refresh token (session tracking) with IP/UA
    let refresh_token_id = Uuid::new_v4();
    let refresh_token_val = Uuid::new_v4().as_simple().to_string();
    let expires_at = Utc::now() + chrono::Duration::days(30);
    let _ = sqlx::query(
        r#"
        INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at, ip_address, user_agent, last_active_at)
        VALUES ($1, $2, $3, $4, $5::inet, $6, NOW())
        "#,
    )
    .bind(refresh_token_id)
    .bind(user.id)
    .bind(hash_token(&refresh_token_val))
    .bind(expires_at)
    .bind(&client_ip)
    .bind(&user_agent)
    .execute(&state.db)
    .await;

    tracing::info!(user_id = %user.id, email = %user.email, "User logged in");

    // Generate RS256 JWT token
    let (token, expires_in) = generate_token(&user, &key_pair)?;

    Ok(Json(AuthResponse {
        token,
        token_type: "Bearer".to_string(),
        expires_in,
        user: UserResponse {
            id: user.id,
            email: user.email,
            name: user.name,
            role: user.role.unwrap_or_else(|| "user".to_string()),
            created_at: user.created_at,
        },
    })
    .into_response())
}

/// Get current authenticated user info
pub async fn get_current_user(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<UserResponse>, ApiError> {
    let user: Option<DbUser> = sqlx::query_as(
        r#"
        SELECT id, email, name, password_hash, role, status, created_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let user = user.ok_or_else(|| ApiError::not_found("User not found"))?;

    Ok(Json(UserResponse {
        id: user.id,
        email: user.email,
        name: user.name,
        role: user.role.unwrap_or_else(|| "user".to_string()),
        created_at: user.created_at,
    }))
}

/// Refresh an expired token
pub async fn refresh_token(
    State(state): State<AppState>,
    Extension(key_pair): Extension<Arc<KeyPair>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<AuthResponse>, ApiError> {
    let user: Option<DbUser> = sqlx::query_as(
        r#"
        SELECT id, email, name, password_hash, role, status, created_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let user = user.ok_or_else(|| ApiError::not_found("User not found"))?;

    // Check if user is still active
    if user.status.as_deref() != Some("active") {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Account is not active",
        ));
    }

    // Update last_active_at on refresh tokens for this user
    let _ = sqlx::query(
        "UPDATE refresh_tokens SET last_active_at = NOW() WHERE user_id = $1 AND expires_at > NOW()",
    )
    .bind(auth.user_id)
    .execute(&state.db)
    .await;

    // Generate RS256 JWT token
    let (token, expires_in) = generate_token(&user, &key_pair)?;

    Ok(Json(AuthResponse {
        token,
        token_type: "Bearer".to_string(),
        expires_in,
        user: UserResponse {
            id: user.id,
            email: user.email,
            name: user.name,
            role: user.role.unwrap_or_else(|| "user".to_string()),
            created_at: user.created_at,
        },
    }))
}

// ---------------------------------------------------------------------------
// New Endpoints: Password Management
// ---------------------------------------------------------------------------

/// Forgot password - generate reset token
///
/// Creates a password reset token, saves the hash to `password_reset_tokens` table.
/// In dev mode, the raw token is returned in the response.
/// In production, an email would be sent instead (not yet implemented).
pub async fn forgot_password(
    State(state): State<AppState>,
    Json(req): Json<ForgotPasswordRequest>,
) -> Result<Json<ForgotPasswordResponse>, ApiError> {
    // Always return success to avoid leaking whether email exists
    let user: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(&req.email)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let is_dev = std::env::var("CANAL_ENV")
        .map(|v| v != "production")
        .unwrap_or(true);

    if let Some((user_id,)) = user {
        let token = generate_random_token();
        let token_hash = hash_token(&token);
        let expires_at = Utc::now() + Duration::hours(1);

        // Invalidate any existing reset tokens for this user
        let _ = sqlx::query(
            "DELETE FROM password_reset_tokens WHERE user_id = $1",
        )
        .bind(user_id)
        .execute(&state.db)
        .await;

        // Insert new reset token
        sqlx::query(
            r#"
            INSERT INTO password_reset_tokens (user_id, token_hash, expires_at)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(user_id)
        .bind(&token_hash)
        .bind(expires_at)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to save reset token: {}", e)))?;

        tracing::info!(user_id = %user_id, "Password reset token generated");

        Ok(Json(ForgotPasswordResponse {
            message: "If the email exists, a password reset link has been sent.".to_string(),
            reset_token: if is_dev { Some(token) } else { None },
        }))
    } else {
        // Return same message to avoid email enumeration
        Ok(Json(ForgotPasswordResponse {
            message: "If the email exists, a password reset link has been sent.".to_string(),
            reset_token: None,
        }))
    }
}

/// Reset password using a valid reset token
pub async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate new password strength
    if let Err(msg) = validate_password_strength(&req.new_password) {
        return Err(ApiError::bad_request(msg));
    }

    let token_hash = hash_token(&req.token);

    // Find valid reset token
    let token_row = sqlx::query(
        r#"
        SELECT user_id, expires_at
        FROM password_reset_tokens
        WHERE token_hash = $1
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let token_row = token_row.ok_or_else(|| {
        ApiError::bad_request("Invalid or expired reset token")
    })?;

    let user_id: Uuid = token_row.try_get("user_id")
        .map_err(|e| ApiError::internal(format!("Failed to read user_id: {}", e)))?;
    let expires_at: chrono::DateTime<Utc> = token_row.try_get("expires_at")
        .map_err(|e| ApiError::internal(format!("Failed to read expires_at: {}", e)))?;

    // Check expiry
    if expires_at < Utc::now() {
        // Clean up expired token
        let _ = sqlx::query("DELETE FROM password_reset_tokens WHERE token_hash = $1")
            .bind(&token_hash)
            .execute(&state.db)
            .await;
        return Err(ApiError::bad_request("Reset token has expired"));
    }

    // Hash new password (CPU-intensive — run off tokio executor)
    let new_pw = req.new_password.clone();
    let new_password_hash = tokio::task::spawn_blocking(move || {
        bcrypt::hash(&new_pw, bcrypt::DEFAULT_COST)
    })
    .await
    .map_err(|e| ApiError::internal(format!("Task join error: {}", e)))?
    .map_err(|e| ApiError::internal(format!("Password hashing error: {}", e)))?;

    // Update password
    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(&new_password_hash)
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update password: {}", e)))?;

    // Delete used reset token
    let _ = sqlx::query("DELETE FROM password_reset_tokens WHERE user_id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await;

    tracing::info!(user_id = %user_id, "Password reset completed");

    Ok(Json(serde_json::json!({
        "message": "Password has been reset successfully"
    })))
}

/// Change password for the authenticated user
pub async fn change_password(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate new password strength
    if let Err(msg) = validate_password_strength(&req.new_password) {
        return Err(ApiError::bad_request(msg));
    }

    // Get current password hash
    let row = sqlx::query("SELECT password_hash FROM users WHERE id = $1")
        .bind(auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let row = row.ok_or_else(|| ApiError::not_found("User not found"))?;

    let current_hash: Option<String> = row.try_get("password_hash").ok();
    let current_hash = current_hash
        .ok_or_else(|| ApiError::bad_request("No password set for this account"))?;

    // Verify current password (CPU-intensive — run off tokio executor)
    let cur_pw = req.current_password.clone();
    let cur_hash = current_hash.clone();
    let valid = tokio::task::spawn_blocking(move || {
        bcrypt::verify(&cur_pw, &cur_hash)
    })
    .await
    .map_err(|e| ApiError::internal(format!("Task join error: {}", e)))?
    .map_err(|e| ApiError::internal(format!("Password verification error: {}", e)))?;

    if !valid {
        return Err(ApiError::bad_request("Current password is incorrect"));
    }

    // Hash and set new password (CPU-intensive — run off tokio executor)
    let new_pw_change = req.new_password.clone();
    let new_hash = tokio::task::spawn_blocking(move || {
        bcrypt::hash(&new_pw_change, bcrypt::DEFAULT_COST)
    })
    .await
    .map_err(|e| ApiError::internal(format!("Task join error: {}", e)))?
    .map_err(|e| ApiError::internal(format!("Password hashing error: {}", e)))?;

    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(&new_hash)
        .bind(auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update password: {}", e)))?;

    tracing::info!(user_id = %auth.user_id, "Password changed");

    Ok(Json(serde_json::json!({
        "message": "Password changed successfully"
    })))
}

// ---------------------------------------------------------------------------
// New Endpoints: Session Management
// ---------------------------------------------------------------------------

/// List active sessions from refresh_tokens table
pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<SessionRecord>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT id, user_id, created_at, expires_at,
               ip_address::text, user_agent, last_active_at
        FROM refresh_tokens
        WHERE user_id = $1 AND expires_at > NOW()
        ORDER BY created_at DESC
        "#,
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let sessions: Vec<SessionRecord> = rows
        .iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").unwrap_or_default();
            SessionRecord {
                id,
                user_id: row.try_get("user_id").unwrap_or_default(),
                created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
                expires_at: row.try_get("expires_at").unwrap_or_else(|_| Utc::now()),
                is_current: false, // Cannot determine current session without token tracking
                ip_address: row.try_get("ip_address").ok().flatten(),
                user_agent: row.try_get("user_agent").ok().flatten(),
                last_active_at: row.try_get("last_active_at").ok().flatten(),
            }
        })
        .collect();

    Ok(Json(sessions))
}

/// Revoke a specific session by ID
pub async fn revoke_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "DELETE FROM refresh_tokens WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("Session not found"));
    }

    tracing::info!(user_id = %auth.user_id, session_id = %id, "Session revoked");

    Ok(Json(serde_json::json!({
        "message": "Session revoked successfully"
    })))
}

/// Revoke all other sessions (keep the current one)
pub async fn revoke_other_sessions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Since we don't track which refresh token maps to the current JWT,
    // we revoke all refresh tokens for this user. The current JWT remains valid
    // until it expires.
    let result = sqlx::query(
        "DELETE FROM refresh_tokens WHERE user_id = $1",
    )
    .bind(auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    tracing::info!(
        user_id = %auth.user_id,
        revoked_count = result.rows_affected(),
        "Other sessions revoked"
    );

    Ok(Json(serde_json::json!({
        "message": "All other sessions have been revoked",
        "revoked_count": result.rows_affected()
    })))
}

// ---------------------------------------------------------------------------
// New Endpoints: Login History
// ---------------------------------------------------------------------------

/// Get login history for the authenticated user (last N entries)
pub async fn login_history(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<LoginHistoryQuery>,
) -> Result<Json<Vec<LoginHistoryEntry>>, ApiError> {
    let limit = query.limit.min(100).max(1);

    let rows = sqlx::query(
        r#"
        SELECT id, user_id, ip_address, user_agent, created_at, success
        FROM login_history
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(auth.user_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let entries: Vec<LoginHistoryEntry> = rows
        .iter()
        .map(|row| LoginHistoryEntry {
            id: row.try_get("id").unwrap_or_default(),
            user_id: row.try_get("user_id").unwrap_or_default(),
            ip_address: row.try_get("ip_address").ok(),
            user_agent: row.try_get("user_agent").ok(),
            login_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
            success: row.try_get("success").unwrap_or(true),
        })
        .collect();

    Ok(Json(entries))
}

// ---------------------------------------------------------------------------
// New Endpoints: TOTP Two-Factor Authentication
// ---------------------------------------------------------------------------

/// Generate TOTP secret for 2FA setup
///
/// Returns a random secret and placeholder provisioning URI.
/// Full QR code generation would require `totp-rs` or similar crate.
pub async fn totp_setup(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<TotpSetupResponse>, ApiError> {
    // Check if TOTP is already enabled
    let row = sqlx::query(
        "SELECT totp_enabled FROM users WHERE id = $1",
    )
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    if let Some(row) = &row {
        let enabled: bool = row.try_get("totp_enabled").unwrap_or(false);
        if enabled {
            return Err(ApiError::bad_request("TOTP is already enabled"));
        }
    }

    // Generate a proper Base32 TOTP secret
    let secret = Secret::generate_secret();
    let secret_base32 = secret.to_encoded().to_string();

    let totp = build_totp(&secret_base32, &auth.email)?;
    let provisioning_uri = totp.get_url();
    let qr_code_base64 = totp.get_qr_base64().map_err(|e| {
        ApiError::internal(format!("Failed to generate QR code: {}", e))
    })?;

    // R4-H20: Store generated secret server-side (totp_enabled stays false until verify)
    // R4-C10: Encrypt TOTP secret at rest using AES-256-GCM
    let encrypted_secret = crate::crypto::encrypt_credential(&secret_base32);
    sqlx::query(
        "UPDATE users SET totp_secret = $1 WHERE id = $2",
    )
    .bind(&encrypted_secret)
    .bind(auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Failed to store TOTP secret: {}", e)))?;

    Ok(Json(TotpSetupResponse {
        secret: secret_base32,
        qr_code_base64,
        provisioning_uri,
    }))
}

/// Verify TOTP code and enable 2FA
///
/// Validates that the code is 6 digits, stores the TOTP secret,
/// and generates recovery codes.
pub async fn totp_verify_setup(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<TotpVerifySetupRequest>,
) -> Result<Json<TotpVerifySetupResponse>, ApiError> {
    // Validate code format: must be exactly 6 digits
    if req.code.len() != 6 || !req.code.chars().all(|c| c.is_ascii_digit()) {
        return Err(ApiError::bad_request("TOTP code must be exactly 6 digits"));
    }

    // R4-H20: Retrieve the server-stored secret instead of accepting it from the client.
    // The secret was stored during totp_setup() — this prevents attackers from
    // supplying their own TOTP secret during verification.
    let row = sqlx::query(
        "SELECT totp_secret, totp_enabled FROM users WHERE id = $1",
    )
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let row = row.ok_or_else(|| ApiError::not_found("User not found"))?;
    let already_enabled: bool = row.try_get("totp_enabled").unwrap_or(false);
    if already_enabled {
        return Err(ApiError::bad_request("TOTP is already enabled"));
    }

    let server_secret_encrypted: Option<String> = row.try_get("totp_secret").ok().flatten();
    let server_secret_encrypted = server_secret_encrypted.ok_or_else(|| {
        ApiError::bad_request("No TOTP setup in progress. Call setup endpoint first.")
    })?;

    // R4-C10: Decrypt TOTP secret from encrypted storage
    let server_secret = crate::crypto::decrypt_credential(&server_secret_encrypted)
        .map_err(|e| ApiError::internal(format!("Failed to decrypt TOTP secret: {}", e)))?;

    // Verify the TOTP code against the server-stored secret
    let totp = build_totp(&server_secret, &auth.email)?;
    let code_valid = totp.check_current(&req.code).map_err(|e| {
        ApiError::internal(format!("TOTP verification error: {}", e))
    })?;
    if !code_valid {
        return Err(ApiError::bad_request(
            "Invalid TOTP code. Please check your authenticator app and try again.",
        ));
    }

    // Generate recovery codes
    let recovery_codes: Vec<String> = (0..8)
        .map(|_| {
            let code = generate_random_token();
            // Format as XXXX-XXXX for readability
            format!("{}-{}", &code[..4], &code[4..8]).to_uppercase()
        })
        .collect();

    // Hash recovery codes for storage as JSONB
    let recovery_hashes: Vec<String> = recovery_codes.iter().map(|c| hash_token(c)).collect();
    let recovery_json: serde_json::Value = serde_json::to_value(&recovery_hashes)
        .map_err(|e| ApiError::internal(format!("Failed to serialize recovery codes: {}", e)))?;

    // Enable TOTP (secret already stored during setup)
    sqlx::query(
        r#"
        UPDATE users
        SET totp_enabled = true, recovery_codes = $1
        WHERE id = $2
        "#,
    )
    .bind(&recovery_json)
    .bind(auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Failed to enable TOTP: {}", e)))?;

    tracing::info!(user_id = %auth.user_id, "TOTP 2FA enabled");

    Ok(Json(TotpVerifySetupResponse { recovery_codes }))
}

/// Disable TOTP 2FA (requires password verification)
pub async fn totp_disable(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<TotpDisableRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify password
    let row = sqlx::query("SELECT password_hash FROM users WHERE id = $1")
        .bind(auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::internal(format!("Database error: {}", e)))?;

    let row = row.ok_or_else(|| ApiError::not_found("User not found"))?;
    let password_hash: Option<String> = row.try_get("password_hash").ok();
    let password_hash =
        password_hash.ok_or_else(|| ApiError::bad_request("No password set for this account"))?;

    let totp_pw = req.password.clone();
    let totp_hash = password_hash.clone();
    let valid = tokio::task::spawn_blocking(move || {
        bcrypt::verify(&totp_pw, &totp_hash)
    })
    .await
    .map_err(|e| ApiError::internal(format!("Task join error: {}", e)))?
    .map_err(|e| ApiError::internal(format!("Password verification error: {}", e)))?;

    if !valid {
        return Err(ApiError::bad_request("Invalid password"));
    }

    // Disable TOTP
    sqlx::query(
        r#"
        UPDATE users
        SET totp_enabled = false, totp_secret = NULL, recovery_codes = NULL
        WHERE id = $1
        "#,
    )
    .bind(auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("Failed to disable TOTP: {}", e)))?;

    tracing::info!(user_id = %auth.user_id, "TOTP 2FA disabled");

    Ok(Json(serde_json::json!({
        "message": "Two-factor authentication has been disabled"
    })))
}

// ---------------------------------------------------------------------------
// Token Generation & Validation
// ---------------------------------------------------------------------------

/// Generate an RS256 JWT access token for a user.
///
/// Uses the shared RSA key pair (loaded from `JWT_PRIVATE_KEY_PEM` in production,
/// ephemeral in development) to sign tokens that the auth middleware can verify.
/// This replaced the legacy HS256 signing path (R4-H16).
fn generate_token(user: &DbUser, key_pair: &Arc<KeyPair>) -> Result<(String, i64), ApiError> {
    let expires_in = TOKEN_EXPIRATION_HOURS * 3600; // seconds
    let role = user.role.clone().unwrap_or_else(|| "user".to_string());

    let permissions = if role == "admin" {
        vec!["*".to_string()]
    } else {
        vec!["read".to_string(), "write".to_string()]
    };

    // Use a deterministic session ID derived from user + timestamp for tracking
    let session_id = Uuid::new_v4().to_string();

    let claims = canal_auth::build_access_claims(
        &user.id.to_string(),
        &user.email,
        user.name.as_deref(),
        &role,
        "free", // default tier; upgraded via billing when applicable
        permissions,
        &session_id,
        None, // org_id
        None, // org_role
    );

    // Override expiry to match the auth routes' 24-hour token lifetime
    // (canal_auth::build_access_claims defaults to 5 minutes for API tokens,
    // but auth route tokens are long-lived login tokens)
    let now = Utc::now().timestamp();
    let claims = canal_auth::AccessClaims {
        exp: now + expires_in,
        iat: now,
        ..claims
    };

    let token = canal_auth::issue_access_token(key_pair, &claims)
        .map_err(|e| ApiError::internal(format!("RS256 token generation error: {}", e)))?;

    Ok((token, expires_in))
}

/// Validate an RS256 JWT token and extract access claims.
///
/// Uses the shared RSA key pair for verification, consistent with token generation (R4-H16).
#[allow(dead_code)]
pub fn validate_token(token: &str, key_pair: &KeyPair) -> Result<canal_auth::AccessClaims, ApiError> {
    canal_auth::verify_access_token(token, key_pair).map_err(|e| {
        tracing::debug!(error = %e, "RS256 token validation failed");
        ApiError::new(StatusCode::UNAUTHORIZED, "Invalid or expired token")
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Password strength tests --

    #[test]
    fn test_password_strength_too_short() {
        assert!(validate_password_strength("Ab1").is_err());
        assert!(validate_password_strength("Abcde1").is_err());
        assert!(validate_password_strength("Short1!").is_err());
    }

    #[test]
    fn test_password_strength_no_uppercase() {
        assert!(validate_password_strength("abcdefg1").is_err());
    }

    #[test]
    fn test_password_strength_no_lowercase() {
        assert!(validate_password_strength("ABCDEFG1").is_err());
    }

    #[test]
    fn test_password_strength_no_digit() {
        assert!(validate_password_strength("Abcdefgh").is_err());
    }

    #[test]
    fn test_password_strength_valid() {
        assert!(validate_password_strength("Abcdefg1").is_ok());
        assert!(validate_password_strength("P@ssw0rd!").is_ok());
        assert!(validate_password_strength("MyStr0ngPwd").is_ok());
    }

    // -- Token generation tests --

    #[test]
    fn test_generate_random_token() {
        let t1 = generate_random_token();
        let t2 = generate_random_token();
        assert_eq!(t1.len(), 32); // UUID hex without dashes
        assert_ne!(t1, t2); // unique
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_token() {
        let token = "test-token-12345";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);
        assert_eq!(hash1, hash2); // deterministic
        assert_eq!(hash1.len(), 64); // SHA-256 hex
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));

        // Different input = different hash
        let hash3 = hash_token("different-token");
        assert_ne!(hash1, hash3);
    }

    // -- JWT tests (RS256) --

    fn test_key_pair() -> Arc<KeyPair> {
        std::env::remove_var("JWT_PRIVATE_KEY_PEM");
        std::env::remove_var("CANAL_ENV");
        canal_auth::load_key_pair()
    }

    #[test]
    fn test_generate_and_validate_token() {
        let kp = test_key_pair();
        let user = DbUser {
            id: Uuid::new_v4(),
            email: "test@example.com".to_string(),
            name: Some("Test User".to_string()),
            password_hash: None,
            role: Some("user".to_string()),
            status: Some("active".to_string()),
            created_at: Utc::now(),
        };

        let (token, expires_in) = generate_token(&user, &kp).expect("token generation failed");
        assert!(!token.is_empty());
        assert_eq!(expires_in, 24 * 3600);

        let claims = validate_token(&token, &kp).expect("token validation failed");
        assert_eq!(claims.sub, user.id.to_string());
        assert_eq!(claims.email, "test@example.com");
        assert_eq!(claims.role, "user");
    }

    #[test]
    fn test_validate_invalid_token() {
        let kp = test_key_pair();
        let result = validate_token("invalid.token.value", &kp);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_empty_token() {
        let kp = test_key_pair();
        let result = validate_token("", &kp);
        assert!(result.is_err());
    }

    // -- Route structure tests --

    #[test]
    fn test_routes_creates_router() {
        // Verify the router can be constructed (compile-time contract)
        let _router = routes();
    }

    // -- Request type tests --

    #[test]
    fn test_register_request_deserialize() {
        let json = r#"{"email":"a@b.com","password":"Test1234","name":"Test"}"#;
        let req: RegisterRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "a@b.com");
        assert_eq!(req.password, "Test1234");
        assert_eq!(req.name, Some("Test".to_string()));
    }

    #[test]
    fn test_login_request_deserialize() {
        let json = r#"{"email":"a@b.com","password":"pass"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "a@b.com");
        assert!(req.totp_code.is_none());
        assert!(req.recovery_code.is_none());
    }

    #[test]
    fn test_login_request_with_totp() {
        let json = r#"{"email":"a@b.com","password":"pass","totp_code":"123456"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "a@b.com");
        assert_eq!(req.totp_code, Some("123456".to_string()));
        assert!(req.recovery_code.is_none());
    }

    #[test]
    fn test_login_request_with_recovery_code() {
        let json =
            r#"{"email":"a@b.com","password":"pass","recovery_code":"ABCD-1234"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.recovery_code, Some("ABCD-1234".to_string()));
        assert!(req.totp_code.is_none());
    }

    #[test]
    fn test_forgot_password_request_deserialize() {
        let json = r#"{"email":"user@test.com"}"#;
        let req: ForgotPasswordRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "user@test.com");
    }

    #[test]
    fn test_change_password_request_deserialize() {
        let json = r#"{"current_password":"old","new_password":"New1234!"}"#;
        let req: ChangePasswordRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.current_password, "old");
        assert_eq!(req.new_password, "New1234!");
    }

    // -- IP/UA extraction tests --

    #[test]
    fn test_extract_client_ip_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.50, 70.41.3.18, 150.172.238.178".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), Some("203.0.113.50".to_string()));
    }

    #[test]
    fn test_extract_client_ip_xff_single() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.1".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), Some("192.168.1.1".to_string()));
    }

    #[test]
    fn test_extract_client_ip_xri_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "10.0.0.1".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), Some("10.0.0.1".to_string()));
    }

    #[test]
    fn test_extract_client_ip_xff_takes_priority() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        headers.insert("x-real-ip", "5.6.7.8".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), Some("1.2.3.4".to_string()));
    }

    #[test]
    fn test_extract_client_ip_none() {
        let headers = HeaderMap::new();
        assert_eq!(extract_client_ip(&headers), None);
    }

    #[test]
    fn test_extract_user_agent_present() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "Mozilla/5.0 (Macintosh)".parse().unwrap());
        assert_eq!(
            extract_user_agent(&headers),
            Some("Mozilla/5.0 (Macintosh)".to_string())
        );
    }

    #[test]
    fn test_extract_user_agent_absent() {
        let headers = HeaderMap::new();
        assert_eq!(extract_user_agent(&headers), None);
    }

    // -- TOTP helper tests --

    #[test]
    fn test_build_totp_valid_secret() {
        let secret = Secret::generate_secret();
        let secret_base32 = secret.to_encoded().to_string();
        let totp = build_totp(&secret_base32, "test@example.com");
        assert!(totp.is_ok());
    }

    #[test]
    fn test_totp_code_generation_and_verification() {
        let secret = Secret::generate_secret();
        let secret_base32 = secret.to_encoded().to_string();
        let totp = build_totp(&secret_base32, "test@example.com").unwrap();

        // Generate current code and verify it passes
        let code = totp.generate_current().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert!(totp.check_current(&code).unwrap());

        // Wrong code should fail
        assert!(!totp.check_current("000000").unwrap_or(false));
    }
}
