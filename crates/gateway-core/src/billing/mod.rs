//! Billing and usage tracking module
//!
//! Provides per-user usage tracking, cost calculation, and billing event persistence.

pub mod gift_card;

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Row};
use std::collections::HashMap;
use std::sync::RwLock;
use uuid::Uuid;

use crate::error::Result;
use crate::llm::router::Usage;

// ---------------------------------------------------------------------------
// Billing Event Types
// ---------------------------------------------------------------------------

/// Type of billing event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BillingEventType {
    /// LLM API call
    LlmRequest,
    /// Tool execution
    ToolExecution,
    /// Code execution in sandbox
    CodeExecution,
    /// Browser automation
    BrowserAction,
    /// File operation
    FileOperation,
    /// Balance top-up
    TopUp,
    /// Credit adjustment
    CreditAdjustment,
}

impl std::fmt::Display for BillingEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LlmRequest => write!(f, "llm_request"),
            Self::ToolExecution => write!(f, "tool_execution"),
            Self::CodeExecution => write!(f, "code_execution"),
            Self::BrowserAction => write!(f, "browser_action"),
            Self::FileOperation => write!(f, "file_operation"),
            Self::TopUp => write!(f, "top_up"),
            Self::CreditAdjustment => write!(f, "credit_adjustment"),
        }
    }
}

// ---------------------------------------------------------------------------
// Billing Event
// ---------------------------------------------------------------------------

/// A billing event record
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BillingEvent {
    pub id: Uuid,
    pub user_id: Uuid,
    pub event_type: String,
    pub pricing_plan_id: Option<String>,
    pub model_profile_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    pub cost_usd: f64,
    pub balance_before: Option<f64>,
    pub balance_after: Option<f64>,
    pub request_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
    pub timestamp: DateTime<Utc>,
}

/// New billing event for insertion
#[derive(Debug, Clone)]
pub struct NewBillingEvent {
    pub user_id: Uuid,
    pub event_type: BillingEventType,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cost_usd: f64,
    pub request_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Usage Summary
// ---------------------------------------------------------------------------

/// Usage summary for a user
#[derive(Debug, Clone, Serialize, Default)]
pub struct UsageSummary {
    pub user_id: Uuid,
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub by_model: HashMap<String, ModelUsage>,
    pub by_provider: HashMap<String, ProviderUsage>,
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: Option<DateTime<Utc>>,
}

/// Usage breakdown by model
#[derive(Debug, Clone, Serialize, Default)]
pub struct ModelUsage {
    pub model: String,
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Usage breakdown by provider
#[derive(Debug, Clone, Serialize, Default)]
pub struct ProviderUsage {
    pub provider: String,
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

// ---------------------------------------------------------------------------
// Token Pricing
// ---------------------------------------------------------------------------

/// Per-model token pricing in USD per million tokens
#[derive(Debug, Clone)]
pub struct TokenPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

// ---------------------------------------------------------------------------
// Daily Usage & Transaction Types
// ---------------------------------------------------------------------------

/// Daily usage data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

/// Top-up / transaction record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopUpRecord {
    pub id: Uuid,
    pub amount_usd: f64,
    pub balance_before: f64,
    pub balance_after: f64,
    pub source: String,
    pub source_reference: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Billing Service
// ---------------------------------------------------------------------------

/// Service for tracking and persisting user billing events
pub struct BillingService {
    db: PgPool,
    /// In-memory cache for fast access (keyed by user_id)
    cache: RwLock<HashMap<Uuid, UsageSummary>>,
    /// Pricing table
    pricing: HashMap<String, TokenPricing>,
}

impl BillingService {
    /// Create a new billing service with database connection
    pub fn new(db: PgPool) -> Self {
        Self {
            db,
            cache: RwLock::new(HashMap::new()),
            pricing: Self::default_pricing(),
        }
    }

    /// Default pricing for known models (USD per million tokens)
    fn default_pricing() -> HashMap<String, TokenPricing> {
        let mut pricing = HashMap::new();

        // Anthropic models
        pricing.insert(
            "claude-sonnet-4".to_string(),
            TokenPricing {
                input_per_million: 3.00,
                output_per_million: 15.00,
            },
        );
        pricing.insert(
            "claude-opus-4".to_string(),
            TokenPricing {
                input_per_million: 15.00,
                output_per_million: 75.00,
            },
        );
        pricing.insert(
            "claude-3-5-sonnet".to_string(),
            TokenPricing {
                input_per_million: 3.00,
                output_per_million: 15.00,
            },
        );
        pricing.insert(
            "claude-3-5-haiku".to_string(),
            TokenPricing {
                input_per_million: 0.80,
                output_per_million: 4.00,
            },
        );

        // OpenAI models
        pricing.insert(
            "gpt-4o".to_string(),
            TokenPricing {
                input_per_million: 2.50,
                output_per_million: 10.00,
            },
        );
        pricing.insert(
            "gpt-4o-mini".to_string(),
            TokenPricing {
                input_per_million: 0.15,
                output_per_million: 0.60,
            },
        );

        // Google models
        pricing.insert(
            "gemini-2.0-flash".to_string(),
            TokenPricing {
                input_per_million: 0.10,
                output_per_million: 0.40,
            },
        );
        pricing.insert(
            "gemini-2.0-pro".to_string(),
            TokenPricing {
                input_per_million: 1.25,
                output_per_million: 5.00,
            },
        );

        // Qwen models
        pricing.insert(
            "qwen3-max".to_string(),
            TokenPricing {
                input_per_million: 1.20,
                output_per_million: 6.00,
            },
        );
        pricing.insert(
            "qwen-turbo".to_string(),
            TokenPricing {
                input_per_million: 0.30,
                output_per_million: 0.60,
            },
        );

        pricing
    }

    /// Find pricing for a model (supports prefix matching)
    fn find_pricing(&self, model: &str) -> Option<&TokenPricing> {
        // Exact match first
        if let Some(p) = self.pricing.get(model) {
            return Some(p);
        }

        // Longest prefix match
        for end in (1..model.len()).rev() {
            if model.is_char_boundary(end) {
                let prefix = &model[..end];
                if let Some(p) = self.pricing.get(prefix) {
                    return Some(p);
                }
            }
        }

        None
    }

    /// Calculate cost for given usage
    pub fn calculate_cost(&self, model: &str, usage: &Usage) -> f64 {
        let input_tokens = usage.prompt_tokens.max(0) as f64;
        let output_tokens = usage.completion_tokens.max(0) as f64;

        self.find_pricing(model)
            .map(|p| {
                (input_tokens * p.input_per_million + output_tokens * p.output_per_million)
                    / 1_000_000.0
            })
            .unwrap_or(0.0)
    }

    /// Record an LLM request billing event
    pub async fn record_llm_usage(
        &self,
        user_id: Uuid,
        model: &str,
        provider: Option<&str>,
        usage: &Usage,
        request_id: Option<Uuid>,
    ) -> Result<BillingEvent> {
        let cost = self.calculate_cost(model, usage);

        let event = NewBillingEvent {
            user_id,
            event_type: BillingEventType::LlmRequest,
            provider: provider.map(String::from),
            model: Some(model.to_string()),
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cost_usd: cost,
            request_id,
            metadata: None,
        };

        self.record_event(event).await
    }

    /// Record a billing event to the database
    pub async fn record_event(&self, event: NewBillingEvent) -> Result<BillingEvent> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let total_tokens = event.input_tokens + event.output_tokens;

        // Insert into database using runtime query
        let record: BillingEvent = sqlx::query_as(
            r#"
            INSERT INTO billing_events (
                id, user_id, event_type, provider, model,
                input_tokens, output_tokens, total_tokens,
                cost_usd, request_id, metadata, timestamp
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING
                id, user_id, event_type, pricing_plan_id, model_profile_id,
                provider, model, input_tokens, output_tokens, total_tokens,
                cost_usd, balance_before, balance_after, request_id,
                metadata, timestamp
            "#,
        )
        .bind(id)
        .bind(event.user_id)
        .bind(event.event_type.to_string())
        .bind(&event.provider)
        .bind(&event.model)
        .bind(event.input_tokens)
        .bind(event.output_tokens)
        .bind(total_tokens)
        .bind(event.cost_usd)
        .bind(event.request_id)
        .bind(&event.metadata)
        .bind(now)
        .fetch_one(&self.db)
        .await?;

        // Update in-memory cache
        self.update_cache(
            event.user_id,
            event.model.as_deref().unwrap_or("unknown"),
            event.provider.as_deref().unwrap_or("unknown"),
            event.input_tokens as u64,
            event.output_tokens as u64,
            event.cost_usd,
        );

        tracing::debug!(
            user_id = %event.user_id,
            model = ?event.model,
            cost = event.cost_usd,
            "Recorded billing event"
        );

        Ok(record)
    }

    /// Update the in-memory cache
    fn update_cache(
        &self,
        user_id: Uuid,
        model: &str,
        provider: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost: f64,
    ) {
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        let summary = cache.entry(user_id).or_insert_with(|| UsageSummary {
            user_id,
            ..Default::default()
        });

        summary.total_requests += 1;
        summary.total_input_tokens += input_tokens;
        summary.total_output_tokens += output_tokens;
        summary.total_cost_usd += cost;

        // Update by_model
        let model_usage = summary
            .by_model
            .entry(model.to_string())
            .or_insert_with(|| ModelUsage {
                model: model.to_string(),
                ..Default::default()
            });
        model_usage.request_count += 1;
        model_usage.input_tokens += input_tokens;
        model_usage.output_tokens += output_tokens;
        model_usage.cost_usd += cost;

        // Update by_provider
        let provider_usage = summary
            .by_provider
            .entry(provider.to_string())
            .or_insert_with(|| ProviderUsage {
                provider: provider.to_string(),
                ..Default::default()
            });
        provider_usage.request_count += 1;
        provider_usage.input_tokens += input_tokens;
        provider_usage.output_tokens += output_tokens;
        provider_usage.cost_usd += cost;
    }

    /// Get usage summary for a user from cache (fast, may be incomplete)
    pub fn get_cached_summary(&self, user_id: Uuid) -> Option<UsageSummary> {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        cache.get(&user_id).cloned()
    }

    /// Get usage summary for a user from database (complete, slower)
    pub async fn get_usage_summary(
        &self,
        user_id: Uuid,
        start_date: Option<DateTime<Utc>>,
        end_date: Option<DateTime<Utc>>,
    ) -> Result<UsageSummary> {
        let start = start_date.unwrap_or_else(|| {
            // Default to beginning of current month
            let now = Utc::now();
            now.date_naive()
                .with_day(1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
        });
        let end = end_date.unwrap_or_else(Utc::now);

        // Get aggregated stats using runtime query
        let stats_row = sqlx::query(
            r#"
            SELECT
                COUNT(*) as request_count,
                COALESCE(SUM(input_tokens), 0) as total_input,
                COALESCE(SUM(output_tokens), 0) as total_output,
                COALESCE(SUM(cost_usd), 0.0) as total_cost
            FROM billing_events
            WHERE user_id = $1
              AND timestamp >= $2
              AND timestamp <= $3
              AND event_type = 'llm_request'
            "#,
        )
        .bind(user_id)
        .bind(start)
        .bind(end)
        .fetch_one(&self.db)
        .await?;

        let request_count: i64 = stats_row.try_get("request_count").unwrap_or(0);
        let total_input: i64 = stats_row.try_get("total_input").unwrap_or(0);
        let total_output: i64 = stats_row.try_get("total_output").unwrap_or(0);
        let total_cost: f64 = stats_row.try_get("total_cost").unwrap_or(0.0);

        // Get breakdown by model
        let model_rows = sqlx::query(
            r#"
            SELECT
                model,
                COUNT(*) as count,
                COALESCE(SUM(input_tokens), 0) as input,
                COALESCE(SUM(output_tokens), 0) as output,
                COALESCE(SUM(cost_usd), 0.0) as cost
            FROM billing_events
            WHERE user_id = $1
              AND timestamp >= $2
              AND timestamp <= $3
              AND event_type = 'llm_request'
            GROUP BY model
            "#,
        )
        .bind(user_id)
        .bind(start)
        .bind(end)
        .fetch_all(&self.db)
        .await?;

        // Get breakdown by provider
        let provider_rows = sqlx::query(
            r#"
            SELECT
                provider,
                COUNT(*) as count,
                COALESCE(SUM(input_tokens), 0) as input,
                COALESCE(SUM(output_tokens), 0) as output,
                COALESCE(SUM(cost_usd), 0.0) as cost
            FROM billing_events
            WHERE user_id = $1
              AND timestamp >= $2
              AND timestamp <= $3
              AND event_type = 'llm_request'
            GROUP BY provider
            "#,
        )
        .bind(user_id)
        .bind(start)
        .bind(end)
        .fetch_all(&self.db)
        .await?;

        let mut by_model = HashMap::new();
        for row in model_rows {
            let model: Option<String> = row.try_get("model").ok();
            if let Some(model) = model {
                let count: i64 = row.try_get("count").unwrap_or(0);
                let input: i64 = row.try_get("input").unwrap_or(0);
                let output: i64 = row.try_get("output").unwrap_or(0);
                let cost: f64 = row.try_get("cost").unwrap_or(0.0);
                by_model.insert(
                    model.clone(),
                    ModelUsage {
                        model,
                        request_count: count as u64,
                        input_tokens: input as u64,
                        output_tokens: output as u64,
                        cost_usd: cost,
                    },
                );
            }
        }

        let mut by_provider = HashMap::new();
        for row in provider_rows {
            let provider: Option<String> = row.try_get("provider").ok();
            if let Some(provider) = provider {
                let count: i64 = row.try_get("count").unwrap_or(0);
                let input: i64 = row.try_get("input").unwrap_or(0);
                let output: i64 = row.try_get("output").unwrap_or(0);
                let cost: f64 = row.try_get("cost").unwrap_or(0.0);
                by_provider.insert(
                    provider.clone(),
                    ProviderUsage {
                        provider,
                        request_count: count as u64,
                        input_tokens: input as u64,
                        output_tokens: output as u64,
                        cost_usd: cost,
                    },
                );
            }
        }

        Ok(UsageSummary {
            user_id,
            total_requests: request_count as u64,
            total_input_tokens: total_input as u64,
            total_output_tokens: total_output as u64,
            total_cost_usd: total_cost,
            by_model,
            by_provider,
            period_start: Some(start),
            period_end: Some(end),
        })
    }

    /// Get billing event history for a user
    pub async fn get_event_history(
        &self,
        user_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<BillingEvent>> {
        let events: Vec<BillingEvent> = sqlx::query_as(
            r#"
            SELECT
                id, user_id, event_type, pricing_plan_id, model_profile_id,
                provider, model, input_tokens, output_tokens, total_tokens,
                cost_usd, balance_before, balance_after, request_id,
                metadata, timestamp
            FROM billing_events
            WHERE user_id = $1
            ORDER BY timestamp DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?;

        Ok(events)
    }

    /// Get daily usage aggregated by day
    pub async fn get_daily_usage(&self, user_id: Uuid, days: i64) -> Result<Vec<DailyUsage>> {
        let since = Utc::now() - chrono::Duration::days(days);
        let rows = sqlx::query(
            r#"
            SELECT
                DATE(timestamp) as day,
                COUNT(*) as requests,
                COALESCE(SUM(input_tokens), 0) as input_tokens,
                COALESCE(SUM(output_tokens), 0) as output_tokens,
                COALESCE(SUM(cost_usd), 0.0) as cost_usd
            FROM billing_events
            WHERE user_id = $1 AND timestamp >= $2 AND event_type = 'llm_request'
            GROUP BY DATE(timestamp)
            ORDER BY day ASC
            "#,
        )
        .bind(user_id)
        .bind(since)
        .fetch_all(&self.db)
        .await?;

        let mut usage = Vec::new();
        for row in rows {
            usage.push(DailyUsage {
                date: row
                    .try_get::<chrono::NaiveDate, _>("day")
                    .map(|d| d.to_string())
                    .unwrap_or_default(),
                requests: row.try_get::<i64, _>("requests").unwrap_or(0),
                input_tokens: row.try_get::<i64, _>("input_tokens").unwrap_or(0),
                output_tokens: row.try_get::<i64, _>("output_tokens").unwrap_or(0),
                cost_usd: row.try_get::<f64, _>("cost_usd").unwrap_or(0.0),
            });
        }
        Ok(usage)
    }

    /// Record a top-up with source tracking
    pub async fn record_topup(
        &self,
        user_id: Uuid,
        amount_usd: f64,
        source: &str,
        source_reference: Option<&str>,
    ) -> Result<f64> {
        let balance_before = self.get_balance(user_id).await?;
        let balance_after = balance_before + amount_usd;

        sqlx::query(
            r#"
            INSERT INTO billing_topups (user_id, amount_usd, balance_before, balance_after, source, source_reference)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(user_id)
        .bind(amount_usd)
        .bind(balance_before)
        .bind(balance_after)
        .bind(source)
        .bind(source_reference)
        .execute(&self.db)
        .await?;

        sqlx::query("UPDATE users SET balance_usd = $1 WHERE id = $2")
            .bind(balance_after)
            .bind(user_id)
            .execute(&self.db)
            .await?;

        Ok(balance_after)
    }

    /// Get top-up / transaction history
    pub async fn get_transactions(
        &self,
        user_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<TopUpRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, amount_usd, balance_before, balance_after,
                   COALESCE(source, 'manual') as source,
                   source_reference, notes, created_at
            FROM billing_topups
            WHERE user_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?;

        let mut records = Vec::new();
        for row in rows {
            records.push(TopUpRecord {
                id: row.try_get("id").unwrap_or_default(),
                amount_usd: row.try_get("amount_usd").unwrap_or(0.0),
                balance_before: row.try_get("balance_before").unwrap_or(0.0),
                balance_after: row.try_get("balance_after").unwrap_or(0.0),
                source: row.try_get("source").unwrap_or_default(),
                source_reference: row.try_get("source_reference").ok(),
                notes: row.try_get("notes").ok(),
                created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(records)
    }

    /// Get user's monthly budget limit
    pub async fn get_budget(&self, user_id: Uuid) -> Result<Option<f64>> {
        let row = sqlx::query("SELECT monthly_budget_limit_usd FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&self.db)
            .await?;

        Ok(row.try_get("monthly_budget_limit_usd").ok())
    }

    /// Set user's monthly budget limit
    pub async fn set_budget(&self, user_id: Uuid, limit: Option<f64>) -> Result<()> {
        sqlx::query("UPDATE users SET monthly_budget_limit_usd = $1 WHERE id = $2")
            .bind(limit)
            .bind(user_id)
            .execute(&self.db)
            .await?;

        Ok(())
    }

    /// Get user's current balance (returns 0.0 if balance column doesn't exist)
    pub async fn get_balance(&self, user_id: Uuid) -> Result<f64> {
        // Try to get balance from users table, default to calculating from events
        let result = sqlx::query(
            r#"
            SELECT
                COALESCE(
                    (SELECT SUM(amount_usd) FROM billing_topups WHERE user_id = $1),
                    0.0
                ) - COALESCE(
                    (SELECT SUM(cost_usd) FROM billing_events WHERE user_id = $1),
                    0.0
                ) as balance
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.db)
        .await?;

        let balance: f64 = result.try_get("balance").unwrap_or(0.0);
        Ok(balance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_calculate_cost() {
        let db = PgPool::connect_lazy("postgres://localhost/test").unwrap();
        let service = BillingService::new(db);

        let usage = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 500_000,
            total_tokens: 1_500_000,
        };

        // Test with known model
        let cost = service.calculate_cost("claude-sonnet-4", &usage);
        // 1M * 3.00 + 0.5M * 15.00 = 3.00 + 7.50 = 10.50
        assert!((cost - 10.50).abs() < 0.01);

        // Test prefix matching
        let cost = service.calculate_cost("claude-sonnet-4-6", &usage);
        assert!((cost - 10.50).abs() < 0.01);

        // Test unknown model (should return 0)
        let cost = service.calculate_cost("unknown-model", &usage);
        assert!((cost - 0.0).abs() < 0.01);
    }
}
