//! Mastodon/ActivityPub channel adapter.
//!
//! Sends toots/replies via the Mastodon REST API and parses incoming
//! notification webhook payloads for mentions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// Mastodon REST API adapter.
///
/// Receives: Mastodon webhook payloads (notification events for mentions).
/// Sends: toots/replies via POST /api/v1/statuses.
pub struct MastodonAdapter {
    /// The Mastodon instance URL (e.g. "https://mastodon.social").
    instance_url: String,
    /// OAuth access token for the Mastodon API.
    access_token: String,
    /// Default visibility for outgoing posts.
    default_visibility: MastodonVisibility,
    /// HTTP client for API calls.
    client: reqwest::Client,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

/// Mastodon post visibility levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MastodonVisibility {
    /// Visible to everyone, shown in public timelines.
    Public,
    /// Visible to everyone, but not shown in public timelines.
    Unlisted,
    /// Visible only to followers.
    Private,
    /// Visible only to mentioned users.
    Direct,
}

impl MastodonVisibility {
    fn as_str(&self) -> &str {
        match self {
            Self::Public => "public",
            Self::Unlisted => "unlisted",
            Self::Private => "private",
            Self::Direct => "direct",
        }
    }
}

impl MastodonAdapter {
    /// Create a new Mastodon adapter.
    ///
    /// `instance_url`: The base URL of the Mastodon instance.
    /// `access_token`: OAuth access token for API authentication.
    pub fn new(instance_url: String, access_token: String) -> Self {
        Self {
            instance_url,
            access_token,
            default_visibility: MastodonVisibility::Unlisted,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Set the default visibility for outgoing posts.
    pub fn with_visibility(mut self, visibility: MastodonVisibility) -> Self {
        self.default_visibility = visibility;
        self
    }

    /// Parse a Mastodon notification webhook payload (mention) into an `IncomingMessage`.
    ///
    /// Expected JSON format:
    /// ```json
    /// {
    ///   "id": "12345",
    ///   "type": "mention",
    ///   "created_at": "2024-01-01T00:00:00.000Z",
    ///   "account": {
    ///     "id": "user123",
    ///     "acct": "alice@mastodon.social",
    ///     "display_name": "Alice"
    ///   },
    ///   "status": {
    ///     "id": "status456",
    ///     "content": "<p>@bot Hello from Mastodon!</p>",
    ///     "spoiler_text": "",
    ///     "visibility": "public",
    ///     "in_reply_to_id": null
    ///   }
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let notification_type = payload.get("type")?.as_str()?;
        if notification_type != "mention" {
            return None;
        }

        let account = payload.get("account")?;
        let user_id = account.get("id")?.as_str()?;
        let acct = account
            .get("acct")
            .and_then(|v| v.as_str())
            .unwrap_or(user_id);
        let display_name = account
            .get("display_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(acct);

        let status = payload.get("status")?;
        let status_id = status.get("id")?.as_str()?;
        let content_html = status.get("content")?.as_str()?;

        // Strip HTML tags for plain text extraction
        let text = strip_html_tags(content_html);
        if text.is_empty() {
            return None;
        }

        let created_at = payload
            .get("created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let spoiler_text = status
            .get("spoiler_text")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let visibility = status
            .get("visibility")
            .and_then(|v| v.as_str())
            .unwrap_or("public");

        let mut metadata = HashMap::new();
        if !spoiler_text.is_empty() {
            metadata.insert(
                "content_warning".to_string(),
                serde_json::Value::String(spoiler_text.to_string()),
            );
        }
        metadata.insert(
            "visibility".to_string(),
            serde_json::Value::String(visibility.to_string()),
        );
        metadata.insert(
            "acct".to_string(),
            serde_json::Value::String(acct.to_string()),
        );

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: status_id.to_string(),
            user_id: user_id.to_string(),
            display_name: display_name.to_string(),
            text,
            timestamp: created_at,
            platform: ChannelPlatform::Mastodon,
            platform_message_id: status_id.to_string(),
            is_group: false,
            metadata,
        })
    }

    /// Post a status (toot) as a reply, optionally with a content warning.
    async fn api_post_status(
        &self,
        in_reply_to_id: &str,
        text: &str,
        content_warning: Option<&str>,
        visibility: Option<MastodonVisibility>,
    ) -> PunchResult<()> {
        let url = format!("{}/api/v1/statuses", self.instance_url);

        let vis = visibility.unwrap_or(self.default_visibility);
        let mut body = serde_json::json!({
            "status": text,
            "in_reply_to_id": in_reply_to_id,
            "visibility": vis.as_str(),
        });

        if let Some(cw) = content_warning {
            body["spoiler_text"] = serde_json::Value::String(cw.to_string());
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "mastodon".to_string(),
                message: format!("failed to post status: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Mastodon post status failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Strip HTML tags from a string for plain text extraction.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result.trim().to_string()
}

#[async_trait]
impl ChannelAdapter for MastodonAdapter {
    fn name(&self) -> &str {
        "mastodon"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Mastodon
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(instance = %self.instance_url, "Mastodon adapter started (webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Mastodon adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.api_post_status(channel_id, message, None, None).await
    }

    fn status(&self) -> ChannelStatus {
        ChannelStatus {
            connected: self.running.load(Ordering::Relaxed),
            started_at: self.started_at.try_read().ok().and_then(|g| *g),
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            last_error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_adapter() -> MastodonAdapter {
        MastodonAdapter::new(
            "https://mastodon.social".to_string(),
            "test-access-token".to_string(),
        )
    }

    #[test]
    fn test_mastodon_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "mastodon");
        assert_eq!(adapter.platform(), ChannelPlatform::Mastodon);
    }

    #[test]
    fn test_parse_mastodon_mention_notification() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "id": "12345",
            "type": "mention",
            "created_at": "2024-01-15T12:00:00.000Z",
            "account": {
                "id": "user123",
                "acct": "alice@mastodon.social",
                "display_name": "Alice"
            },
            "status": {
                "id": "status456",
                "content": "<p>@bot Hello from Mastodon!</p>",
                "spoiler_text": "",
                "visibility": "public",
                "in_reply_to_id": null
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Mastodon);
        assert_eq!(msg.user_id, "user123");
        assert_eq!(msg.display_name, "Alice");
        assert_eq!(msg.text, "@bot Hello from Mastodon!");
        assert_eq!(msg.platform_message_id, "status456");
    }

    #[test]
    fn test_parse_mastodon_with_content_warning() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "id": "67890",
            "type": "mention",
            "created_at": "2024-01-15T12:00:00.000Z",
            "account": {
                "id": "user456",
                "acct": "bob@instance.example",
                "display_name": "Bob"
            },
            "status": {
                "id": "status789",
                "content": "<p>@bot Sensitive content here</p>",
                "spoiler_text": "Spoiler alert",
                "visibility": "unlisted"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(
            msg.metadata.get("content_warning").unwrap(),
            &serde_json::Value::String("Spoiler alert".to_string())
        );
        assert_eq!(
            msg.metadata.get("visibility").unwrap(),
            &serde_json::Value::String("unlisted".to_string())
        );
    }

    #[test]
    fn test_parse_mastodon_non_mention_ignored() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "id": "99999",
            "type": "favourite",
            "created_at": "2024-01-15T12:00:00.000Z",
            "account": {
                "id": "user789",
                "acct": "charlie",
                "display_name": "Charlie"
            },
            "status": {
                "id": "status111",
                "content": "<p>Some toot</p>"
            }
        });

        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(
            strip_html_tags("<p>Hello <b>world</b>!</p>"),
            "Hello world!"
        );
        assert_eq!(strip_html_tags("plain text"), "plain text");
        assert_eq!(strip_html_tags("<p></p>"), "");
    }

    #[tokio::test]
    async fn test_mastodon_adapter_start_stop() {
        let adapter = make_adapter();

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        let status = adapter.status();
        assert!(status.connected);
        assert!(status.started_at.is_some());

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
