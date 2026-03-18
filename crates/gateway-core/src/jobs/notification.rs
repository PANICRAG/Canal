//! Webhook notification for job lifecycle events.

use async_trait::async_trait;
use tracing::instrument;

use super::types::Job;

/// Trait for notifying external systems about job lifecycle events.
#[async_trait]
pub trait JobNotifier: Send + Sync {
    /// Called when a job completes successfully.
    async fn on_completed(&self, job: &Job);
    /// Called when a job fails.
    async fn on_failed(&self, job: &Job);
}

/// Webhook-based notifier that POSTs JSON payloads to a URL.
pub struct WebhookNotifier {
    client: reqwest::Client,
    default_url: Option<String>,
    enabled_events: Vec<String>,
}

impl WebhookNotifier {
    /// Create a new webhook notifier.
    pub fn new(default_url: Option<String>, enabled_events: Vec<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            default_url,
            enabled_events,
        }
    }

    fn should_notify(&self, event_name: &str) -> bool {
        self.enabled_events.iter().any(|e| e == event_name)
    }

    fn resolve_url(&self, job: &Job) -> Option<String> {
        job.notify_webhook
            .clone()
            .or_else(|| self.default_url.clone())
    }

    /// Check if a URL is safe for webhook delivery (rejects private/loopback addresses).
    /// R2-H1: Extended to cover IPv6-mapped addresses, fd00::/8, fe80::/10, and more
    /// precise 172.16.0.0/12 range validation.
    fn is_safe_webhook_url(url: &str) -> bool {
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        if !matches!(parsed.scheme(), "https" | "http") {
            return false;
        }
        match parsed.host_str() {
            None => false,
            Some(host) => {
                let lower = host.to_lowercase();
                // Reject loopback and private network addresses
                if matches!(
                    lower.as_str(),
                    "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "[::1]" | "[::0]"
                ) {
                    return false;
                }
                // IPv4 private/reserved ranges
                if lower.starts_with("10.")
                    || lower.starts_with("192.168.")
                    || lower.starts_with("169.254.")
                {
                    return false;
                }
                // 172.16.0.0/12 = 172.16.x.x through 172.31.x.x
                if lower.starts_with("172.") {
                    if let Some(second_octet) = lower
                        .strip_prefix("172.")
                        .and_then(|s| s.split('.').next())
                        .and_then(|s| s.parse::<u8>().ok())
                    {
                        if (16..=31).contains(&second_octet) {
                            return false;
                        }
                    }
                }
                // IPv6 loopback and private ranges
                if lower.starts_with("[::ffff:") // IPv6-mapped IPv4
                    || lower.starts_with("[fd")   // fd00::/8 unique local
                    || lower.starts_with("[fe80") // fe80::/10 link-local
                    || lower.starts_with("[fc")   // fc00::/7 unique local
                    || lower.starts_with("::ffff:")
                    || lower.starts_with("fd")
                    || lower.starts_with("fe80")
                    || lower.starts_with("fc")
                {
                    return false;
                }
                // Reject internal/local suffixes
                if lower.ends_with(".local") || lower.ends_with(".internal") {
                    return false;
                }
                true
            }
        }
    }

    #[instrument(skip(self, job), fields(job_id = %job.id, event = %event))]
    async fn send_notification(&self, job: &Job, event: &str) {
        let url = match self.resolve_url(job) {
            Some(url) if !url.is_empty() => url,
            _ => return,
        };

        // SSRF protection: reject private/loopback/metadata URLs
        if !Self::is_safe_webhook_url(&url) {
            tracing::warn!(url = %url, "Webhook URL rejected: private or invalid address");
            return;
        }

        // Truncate at char boundary to avoid panic on multi-byte text (CJK, emoji)
        let message_preview = if job.input.message.chars().count() > 100 {
            let byte_end = job
                .input
                .message
                .char_indices()
                .nth(100)
                .map(|(i, _)| i)
                .unwrap_or(job.input.message.len());
            &job.input.message[..byte_end]
        } else {
            &job.input.message
        };

        let payload = serde_json::json!({
            "event": event,
            "job_id": job.id.to_string(),
            "status": job.status,
            "message_preview": message_preview,
            "completed_at": job.completed_at,
        });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                tracing::info!(
                    status = %resp.status(),
                    "Webhook notification sent"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to send webhook notification");
            }
        }
    }
}

#[async_trait]
impl JobNotifier for WebhookNotifier {
    async fn on_completed(&self, job: &Job) {
        if self.should_notify("completed") {
            self.send_notification(job, "completed").await;
        }
    }

    async fn on_failed(&self, job: &Job) {
        if self.should_notify("failed") {
            self.send_notification(job, "failed").await;
        }
    }
}
