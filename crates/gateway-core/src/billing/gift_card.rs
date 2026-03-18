//! Gift Card Service
//!
//! Production-grade gift card generation, redemption, and management.
//! Supports batch generation, transactional redemption, and admin management.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Row};
use uuid::Uuid;

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Safe alphabet for gift card codes (excludes 0/O/I/1/L to avoid confusion)
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// Gift card status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GiftCardStatus {
    Active,
    Redeemed,
    Expired,
    Disabled,
}

impl std::fmt::Display for GiftCardStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Redeemed => write!(f, "redeemed"),
            Self::Expired => write!(f, "expired"),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

/// Gift card record
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GiftCard {
    pub id: Uuid,
    pub code: String,
    pub amount_usd: f64,
    pub currency: String,
    pub status: String,
    pub created_by: Option<Uuid>,
    pub redeemed_by: Option<Uuid>,
    pub redeemed_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub batch_id: Option<Uuid>,
    pub notes: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Result of a successful gift card redemption
#[derive(Debug, Clone, Serialize)]
pub struct RedeemResult {
    pub new_balance: f64,
    pub amount_credited: f64,
    pub card_id: Uuid,
    pub card_code: String,
}

/// Gift card statistics
#[derive(Debug, Clone, Serialize, Default)]
pub struct GiftCardStats {
    pub total_cards: i64,
    pub active_cards: i64,
    pub redeemed_cards: i64,
    pub expired_cards: i64,
    pub disabled_cards: i64,
    pub total_value_usd: f64,
    pub redeemed_value_usd: f64,
}

/// Request to generate gift cards
#[derive(Debug, Deserialize)]
pub struct GenerateRequest {
    pub count: u32,
    pub amount_usd: f64,
    pub expires_days: Option<i64>,
    pub notes: Option<String>,
}

// ---------------------------------------------------------------------------
// Gift Card Service
// ---------------------------------------------------------------------------

/// Service for managing gift cards
pub struct GiftCardService {
    db: PgPool,
}

impl GiftCardService {
    /// Create a new gift card service
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Generate a single gift card code in XXXX-XXXX-XXXX-XXXX format
    /// Uses UUID v4 bytes as entropy source (cryptographically random)
    pub fn generate_code() -> String {
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let mut code = String::with_capacity(19);
        for i in 0..16 {
            if i > 0 && i % 4 == 0 {
                code.push('-');
            }
            let idx = (bytes[i] as usize) % CODE_ALPHABET.len();
            code.push(CODE_ALPHABET[idx] as char);
        }
        code
    }

    /// Validate that a code matches XXXX-XXXX-XXXX-XXXX format
    pub fn validate_code_format(code: &str) -> bool {
        let parts: Vec<&str> = code.split('-').collect();
        if parts.len() != 4 {
            return false;
        }
        parts.iter().all(|part| {
            part.len() == 4
                && part
                    .bytes()
                    .all(|b| CODE_ALPHABET.contains(&b.to_ascii_uppercase()))
        })
    }

    /// Batch generate gift cards
    pub async fn generate_cards(
        &self,
        count: u32,
        amount_usd: f64,
        expires_days: Option<i64>,
        notes: Option<&str>,
        admin_id: Uuid,
    ) -> Result<Vec<GiftCard>> {
        let batch_id = Uuid::new_v4();
        let expires_at = expires_days.map(|days| Utc::now() + Duration::days(days));
        let mut cards = Vec::with_capacity(count as usize);

        for _ in 0..count {
            let code = Self::generate_code();
            let card: GiftCard = sqlx::query_as(
                r#"
                INSERT INTO gift_cards (code, amount_usd, created_by, expires_at, batch_id, notes)
                VALUES ($1, $2, $3, $4, $5, $6)
                RETURNING id, code, amount_usd, currency, status, created_by, redeemed_by,
                          redeemed_at, expires_at, batch_id, notes, metadata, created_at, updated_at
                "#,
            )
            .bind(&code)
            .bind(amount_usd)
            .bind(admin_id)
            .bind(expires_at)
            .bind(batch_id)
            .bind(notes)
            .fetch_one(&self.db)
            .await?;

            cards.push(card);
        }

        tracing::info!(
            count = count,
            amount_usd = amount_usd,
            batch_id = %batch_id,
            admin_id = %admin_id,
            "Generated gift cards"
        );

        Ok(cards)
    }

    /// Redeem a gift card — full transactional operation
    pub async fn redeem_card(&self, code: &str, user_id: Uuid) -> Result<RedeemResult> {
        let code = code.trim().to_uppercase();

        if !Self::validate_code_format(&code) {
            return Err(Error::InvalidInput(
                "Invalid gift card code format. Expected XXXX-XXXX-XXXX-XXXX".to_string(),
            ));
        }

        // Use a transaction for atomicity
        let mut tx = self.db.begin().await?;

        // Lock the card row for update
        let card_row = sqlx::query(
            r#"
            SELECT id, code, amount_usd, status, expires_at
            FROM gift_cards
            WHERE code = $1
            FOR UPDATE
            "#,
        )
        .bind(&code)
        .fetch_optional(&mut *tx)
        .await?;

        let card_row =
            card_row.ok_or_else(|| Error::NotFound("Gift card not found".to_string()))?;

        let card_id: Uuid = card_row
            .try_get("id")
            .map_err(|e| Error::Internal(format!("Failed to read card id: {e}")))?;
        let amount_usd: f64 = card_row
            .try_get("amount_usd")
            .map_err(|e| Error::Internal(format!("Failed to read amount: {e}")))?;
        let status: String = card_row
            .try_get("status")
            .map_err(|e| Error::Internal(format!("Failed to read status: {e}")))?;
        let expires_at: Option<DateTime<Utc>> = card_row.try_get("expires_at").ok();

        // Validate card status
        if status != "active" {
            let msg = match status.as_str() {
                "redeemed" => "This gift card has already been redeemed",
                "expired" => "This gift card has expired",
                "disabled" => "This gift card has been disabled",
                _ => "This gift card is not available",
            };
            return Err(Error::InvalidInput(msg.to_string()));
        }

        // Check expiry
        if let Some(exp) = expires_at {
            if exp < Utc::now() {
                // Mark as expired
                sqlx::query("UPDATE gift_cards SET status = 'expired' WHERE id = $1")
                    .bind(card_id)
                    .execute(&mut *tx)
                    .await?;
                return Err(Error::InvalidInput(
                    "This gift card has expired".to_string(),
                ));
            }
        }

        // Get current balance
        let balance_row =
            sqlx::query("SELECT COALESCE(balance_usd, 0.0) as balance FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_one(&mut *tx)
                .await?;
        let balance_before: f64 = balance_row.try_get("balance").unwrap_or(0.0);
        let new_balance = balance_before + amount_usd;

        // Update user balance
        sqlx::query("UPDATE users SET balance_usd = $1 WHERE id = $2")
            .bind(new_balance)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

        // Insert top-up record
        sqlx::query(
            r#"
            INSERT INTO billing_topups (user_id, amount_usd, balance_before, balance_after, source, source_reference, notes)
            VALUES ($1, $2, $3, $4, 'gift_card', $5, $6)
            "#,
        )
        .bind(user_id)
        .bind(amount_usd)
        .bind(balance_before)
        .bind(new_balance)
        .bind(card_id.to_string())
        .bind(format!("Gift card redeemed: {}", code))
        .execute(&mut *tx)
        .await?;

        // Update card status
        sqlx::query(
            r#"
            UPDATE gift_cards
            SET status = 'redeemed', redeemed_by = $1, redeemed_at = $2, updated_at = $2
            WHERE id = $3
            "#,
        )
        .bind(user_id)
        .bind(Utc::now())
        .bind(card_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        tracing::info!(
            user_id = %user_id,
            card_code = %code,
            amount = amount_usd,
            new_balance = new_balance,
            "Gift card redeemed"
        );

        Ok(RedeemResult {
            new_balance,
            amount_credited: amount_usd,
            card_id,
            card_code: code,
        })
    }

    /// Get a gift card by code
    pub async fn get_card(&self, code: &str) -> Result<Option<GiftCard>> {
        let card: Option<GiftCard> = sqlx::query_as(
            r#"
            SELECT id, code, amount_usd, currency, status, created_by, redeemed_by,
                   redeemed_at, expires_at, batch_id, notes, metadata, created_at, updated_at
            FROM gift_cards WHERE code = $1
            "#,
        )
        .bind(code.trim().to_uppercase())
        .fetch_optional(&self.db)
        .await?;

        Ok(card)
    }

    /// List gift cards with optional filters
    pub async fn list_cards(
        &self,
        status: Option<&str>,
        batch_id: Option<Uuid>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<GiftCard>> {
        let cards: Vec<GiftCard> = sqlx::query_as(
            r#"
            SELECT id, code, amount_usd, currency, status, created_by, redeemed_by,
                   redeemed_at, expires_at, batch_id, notes, metadata, created_at, updated_at
            FROM gift_cards
            WHERE ($1::TEXT IS NULL OR status = $1)
              AND ($2::UUID IS NULL OR batch_id = $2)
            ORDER BY created_at DESC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(status)
        .bind(batch_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?;

        Ok(cards)
    }

    /// Get gift card statistics
    pub async fn get_stats(&self) -> Result<GiftCardStats> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) as total,
                COUNT(*) FILTER (WHERE status = 'active') as active,
                COUNT(*) FILTER (WHERE status = 'redeemed') as redeemed,
                COUNT(*) FILTER (WHERE status = 'expired') as expired,
                COUNT(*) FILTER (WHERE status = 'disabled') as disabled,
                COALESCE(SUM(amount_usd), 0.0) as total_value,
                COALESCE(SUM(amount_usd) FILTER (WHERE status = 'redeemed'), 0.0) as redeemed_value
            FROM gift_cards
            "#,
        )
        .fetch_one(&self.db)
        .await?;

        Ok(GiftCardStats {
            total_cards: row.try_get("total").unwrap_or(0),
            active_cards: row.try_get("active").unwrap_or(0),
            redeemed_cards: row.try_get("redeemed").unwrap_or(0),
            expired_cards: row.try_get("expired").unwrap_or(0),
            disabled_cards: row.try_get("disabled").unwrap_or(0),
            total_value_usd: row.try_get("total_value").unwrap_or(0.0),
            redeemed_value_usd: row.try_get("redeemed_value").unwrap_or(0.0),
        })
    }

    /// Disable a gift card (admin action)
    pub async fn disable_card(&self, code: &str) -> Result<GiftCard> {
        let card: GiftCard = sqlx::query_as(
            r#"
            UPDATE gift_cards
            SET status = 'disabled', updated_at = now()
            WHERE code = $1 AND status = 'active'
            RETURNING id, code, amount_usd, currency, status, created_by, redeemed_by,
                      redeemed_at, expires_at, batch_id, notes, metadata, created_at, updated_at
            "#,
        )
        .bind(code.trim().to_uppercase())
        .fetch_optional(&self.db)
        .await?
        .ok_or_else(|| Error::NotFound("Active gift card not found with this code".to_string()))?;

        tracing::info!(code = %code, "Gift card disabled");
        Ok(card)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_code_format() {
        let code = GiftCardService::generate_code();
        assert_eq!(code.len(), 19); // 16 chars + 3 dashes
        assert!(GiftCardService::validate_code_format(&code));

        let parts: Vec<&str> = code.split('-').collect();
        assert_eq!(parts.len(), 4);
        for part in parts {
            assert_eq!(part.len(), 4);
        }
    }

    #[test]
    fn test_generate_codes_unique() {
        let codes: Vec<String> = (0..100).map(|_| GiftCardService::generate_code()).collect();
        let unique: std::collections::HashSet<&String> = codes.iter().collect();
        assert_eq!(
            codes.len(),
            unique.len(),
            "Generated codes should be unique"
        );
    }

    #[test]
    fn test_generate_code_no_confusing_chars() {
        for _ in 0..100 {
            let code = GiftCardService::generate_code();
            let clean = code.replace('-', "");
            assert!(
                !clean.contains('0')
                    && !clean.contains('O')
                    && !clean.contains('I')
                    && !clean.contains('1')
                    && !clean.contains('L'),
                "Code should not contain confusing characters: {code}"
            );
        }
    }

    #[test]
    fn test_validate_code_format_valid() {
        assert!(GiftCardService::validate_code_format("ABCD-EFGH-JKMN-PQRS"));
        assert!(GiftCardService::validate_code_format("2345-6789-ABCD-EFGH"));
        assert!(GiftCardService::validate_code_format("abcd-efgh-jkmn-pqrs")); // lowercase ok
    }

    #[test]
    fn test_validate_code_format_invalid() {
        assert!(!GiftCardService::validate_code_format("ABCD-EFGH-JKMN")); // too few parts
        assert!(!GiftCardService::validate_code_format(
            "ABCDE-FGHI-JKMN-PQRS"
        )); // 5 chars
        assert!(!GiftCardService::validate_code_format("ABCD")); // single part
        assert!(!GiftCardService::validate_code_format("")); // empty
        assert!(!GiftCardService::validate_code_format(
            "ABCD-EFGH-JKMN-PQRS-TUVW"
        )); // 5 parts
    }

    #[test]
    fn test_gift_card_status_display() {
        assert_eq!(GiftCardStatus::Active.to_string(), "active");
        assert_eq!(GiftCardStatus::Redeemed.to_string(), "redeemed");
        assert_eq!(GiftCardStatus::Expired.to_string(), "expired");
        assert_eq!(GiftCardStatus::Disabled.to_string(), "disabled");
    }
}
