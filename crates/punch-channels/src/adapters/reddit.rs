//! Reddit channel adapter via the Reddit API.
//!
//! Uses OAuth2 for authentication and supports posting comments/replies
//! and parsing incoming mentions/messages.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const REDDIT_API_BASE: &str = "https://oauth.reddit.com";
const REDDIT_TOKEN_URL: &str = "https://www.reddit.com/api/v1/access_token";
const REDDIT_USER_AGENT: &str = "punch-agent-os:v0.1.0 (by /u/punch-bot)";

/// Reddit API adapter.
///
/// Receives: Reddit webhook/polling payloads for mentions and messages.
/// Sends: comments and replies via the Reddit API.
pub struct RedditAdapter {
    /// OAuth2 client ID.
    client_id: String,
    /// OAuth2 client secret.
    client_secret: String,
    /// OAuth2 refresh token for obtaining access tokens.
    refresh_token: String,
    /// Target subreddit (without the "r/" prefix).
    #[allow(dead_code)]
    subreddit: String,
    /// Current OAuth2 access token.
    access_token: RwLock<String>,
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

impl RedditAdapter {
    /// Create a new Reddit adapter.
    ///
    /// `client_id`: OAuth2 client ID from Reddit app settings.
    /// `client_secret`: OAuth2 client secret.
    /// `refresh_token`: Long-lived refresh token for obtaining access tokens.
    /// `subreddit`: Target subreddit name (without "r/" prefix).
    pub fn new(
        client_id: String,
        client_secret: String,
        refresh_token: String,
        subreddit: String,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            refresh_token,
            subreddit,
            access_token: RwLock::new(String::new()),
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Refresh the OAuth2 access token using the refresh token.
    pub async fn refresh_access_token(&self) -> PunchResult<()> {
        let resp = self
            .client
            .post(REDDIT_TOKEN_URL)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .header("User-Agent", REDDIT_USER_AGENT)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &self.refresh_token),
            ])
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "reddit".to_string(),
                message: format!("failed to refresh token: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(PunchError::Channel {
                channel: "reddit".to_string(),
                message: format!("token refresh failed ({status}): {body_text}"),
            });
        }

        let body: serde_json::Value =
            resp.json().await.map_err(|e| PunchError::Channel {
                channel: "reddit".to_string(),
                message: format!("failed to parse token response: {e}"),
            })?;

        let token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PunchError::Channel {
                channel: "reddit".to_string(),
                message: "no access_token in response".to_string(),
            })?;

        *self.access_token.write().await = token.to_string();
        info!("Reddit OAuth2 access token refreshed");
        Ok(())
    }

    /// Parse a Reddit webhook/notification payload into an `IncomingMessage`.
    ///
    /// Handles both comment mentions and private messages.
    ///
    /// Expected JSON format (comment mention):
    /// ```json
    /// {
    ///   "kind": "t1",
    ///   "data": {
    ///     "id": "comment_id",
    ///     "author": "username",
    ///     "body": "u/punch-bot do something",
    ///     "subreddit": "test",
    ///     "link_id": "t3_post123",
    ///     "parent_id": "t1_parent456",
    ///     "created_utc": 1700000000.0
    ///   }
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let kind = payload.get("kind")?.as_str()?;
        let data = payload.get("data")?;

        let author = data.get("author")?.as_str()?;
        let body = data.get("body")?.as_str()?;
        if body.is_empty() {
            return None;
        }

        let thing_id = data.get("id")?.as_str()?;
        let created_utc = data
            .get("created_utc")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let timestamp =
            DateTime::from_timestamp(created_utc as i64, 0).unwrap_or_else(Utc::now);

        let (channel_id, is_group) = match kind {
            // t1 = comment, t4 = private message
            "t1" => {
                let link_id = data
                    .get("link_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(thing_id);
                (link_id.to_string(), true)
            }
            "t4" => (author.to_string(), false),
            _ => return None,
        };

        let mut metadata = HashMap::new();
        if let Some(subreddit) = data.get("subreddit").and_then(|v| v.as_str()) {
            metadata.insert(
                "subreddit".to_string(),
                serde_json::Value::String(subreddit.to_string()),
            );
        }
        if let Some(parent_id) = data.get("parent_id").and_then(|v| v.as_str()) {
            metadata.insert(
                "parent_id".to_string(),
                serde_json::Value::String(parent_id.to_string()),
            );
        }

        let full_id = format!("{}_{}", kind, thing_id);

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id,
            user_id: author.to_string(),
            display_name: author.to_string(),
            text: body.to_string(),
            timestamp,
            platform: ChannelPlatform::Reddit,
            platform_message_id: full_id,
            is_group,
            metadata,
        })
    }

    /// Post a comment as a reply via the Reddit API.
    async fn api_post_comment(&self, parent_fullname: &str, text: &str) -> PunchResult<()> {
        let url = format!("{}/api/comment", REDDIT_API_BASE);
        let token = self.access_token.read().await.clone();

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", REDDIT_USER_AGENT)
            .form(&[("thing_id", parent_fullname), ("text", text)])
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "reddit".to_string(),
                message: format!("failed to post comment: {e}"),
            })?;

        let status = resp.status();

        // Check rate limit headers
        if let Some(remaining) = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            && let Ok(remaining_f) = remaining.parse::<f64>()
            && remaining_f < 5.0
        {
            warn!(remaining = %remaining, "Reddit rate limit nearly exhausted");
        }

        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Reddit post comment failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for RedditAdapter {
    fn name(&self) -> &str {
        "reddit"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Reddit
    }

    async fn start(&self) -> PunchResult<()> {
        self.refresh_access_token().await?;
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("Reddit adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Reddit adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.api_post_comment(channel_id, message).await
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

    fn make_adapter() -> RedditAdapter {
        RedditAdapter::new(
            "test-client-id".to_string(),
            "test-client-secret".to_string(),
            "test-refresh-token".to_string(),
            "testsubreddit".to_string(),
        )
    }

    #[test]
    fn test_reddit_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "reddit");
        assert_eq!(adapter.platform(), ChannelPlatform::Reddit);
    }

    #[test]
    fn test_parse_reddit_comment_mention() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "kind": "t1",
            "data": {
                "id": "abc123",
                "author": "testuser",
                "body": "u/punch-bot help me with this",
                "subreddit": "testsubreddit",
                "link_id": "t3_post789",
                "parent_id": "t1_parent456",
                "created_utc": 1700000000.0
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Reddit);
        assert_eq!(msg.user_id, "testuser");
        assert_eq!(msg.text, "u/punch-bot help me with this");
        assert_eq!(msg.channel_id, "t3_post789");
        assert!(msg.is_group);
        assert_eq!(
            msg.metadata.get("subreddit").unwrap(),
            &serde_json::Value::String("testsubreddit".to_string())
        );
    }

    #[test]
    fn test_parse_reddit_private_message() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "kind": "t4",
            "data": {
                "id": "msg789",
                "author": "alice",
                "body": "Hey bot, what's up?",
                "created_utc": 1700000000.0
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert!(!msg.is_group);
        assert_eq!(msg.channel_id, "alice");
        assert_eq!(msg.platform_message_id, "t4_msg789");
    }

    #[test]
    fn test_parse_reddit_empty_body() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "kind": "t1",
            "data": {
                "id": "abc",
                "author": "user",
                "body": "",
                "subreddit": "test",
                "link_id": "t3_x",
                "created_utc": 1700000000.0
            }
        });

        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_reddit_unknown_kind() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "kind": "t3",
            "data": {
                "id": "post123",
                "author": "user",
                "body": "A submission",
                "created_utc": 1700000000.0
            }
        });

        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }
}
