//! Discord channel adapter (webhook-based).
//!
//! Receives messages via a webhook endpoint (POST /api/channels/discord/webhook)
//! and sends responses back via Discord webhook URLs.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage, split_message};

const DISCORD_MSG_LIMIT: usize = 2000;

/// Discord channel adapter using webhooks for both receiving and sending.
///
/// Receives: incoming messages via POST to the Arena webhook endpoint.
/// Sends: responses via Discord webhook URL.
pub struct DiscordAdapter {
    /// Bot token for Discord API calls.
    bot_token: String,
    /// Webhook URL for sending messages.
    webhook_url: Option<String>,
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

impl DiscordAdapter {
    /// Create a new Discord adapter.
    ///
    /// `bot_token`: Discord bot token (read from env by caller).
    /// `webhook_url`: Optional webhook URL for sending messages.
    pub fn new(bot_token: String, webhook_url: Option<String>) -> Self {
        Self {
            bot_token,
            webhook_url,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Parse a Discord webhook payload into an IncomingMessage.
    ///
    /// Expected JSON format:
    /// ```json
    /// {
    ///   "channel_id": "123456",
    ///   "author": { "id": "789", "username": "alice" },
    ///   "content": "Hello!",
    ///   "id": "msg123",
    ///   "guild_id": "guild456"  // optional
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let author = payload.get("author")?;
        let author_id = author["id"].as_str()?;

        // Skip bot messages
        if author["bot"].as_bool() == Some(true) {
            return None;
        }

        let content = payload["content"].as_str().unwrap_or("");
        if content.is_empty() {
            return None;
        }

        let channel_id = payload["channel_id"].as_str()?;
        let message_id = payload["id"].as_str().unwrap_or("0");
        let username = author["username"].as_str().unwrap_or("Unknown");
        let is_group = payload["guild_id"].as_str().is_some();

        let timestamp = payload["timestamp"]
            .as_str()
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: channel_id.to_string(),
            user_id: author_id.to_string(),
            display_name: username.to_string(),
            text: content.to_string(),
            timestamp,
            platform: ChannelPlatform::Discord,
            platform_message_id: message_id.to_string(),
            is_group,
            metadata: std::collections::HashMap::new(),
        })
    }

    /// Send a message via Discord REST API to a specific channel.
    async fn api_send_message(&self, channel_id: &str, text: &str) -> PunchResult<()> {
        let url = format!(
            "https://discord.com/api/v10/channels/{}/messages",
            channel_id
        );

        let chunks = split_message(text, DISCORD_MSG_LIMIT);
        for chunk in chunks {
            let body = serde_json::json!({ "content": chunk });
            let resp = self
                .client
                .post(&url)
                .header("Authorization", format!("Bot {}", self.bot_token))
                .json(&body)
                .send()
                .await
                .map_err(|e| PunchError::Channel {
                    channel: "discord".to_string(),
                    message: format!("failed to send message: {e}"),
                })?;

            if !resp.status().is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                warn!("Discord sendMessage failed: {body_text}");
            }
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Send a message via webhook URL.
    async fn webhook_send(&self, text: &str) -> PunchResult<()> {
        let webhook_url = self
            .webhook_url
            .as_ref()
            .ok_or_else(|| PunchError::Channel {
                channel: "discord".to_string(),
                message: "no webhook URL configured".to_string(),
            })?;

        let chunks = split_message(text, DISCORD_MSG_LIMIT);
        for chunk in chunks {
            let body = serde_json::json!({ "content": chunk });
            let resp = self
                .client
                .post(webhook_url)
                .json(&body)
                .send()
                .await
                .map_err(|e| PunchError::Channel {
                    channel: "discord".to_string(),
                    message: format!("webhook send failed: {e}"),
                })?;

            if !resp.status().is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                warn!("Discord webhook send failed: {body_text}");
            }
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for DiscordAdapter {
    fn name(&self) -> &str {
        "discord"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Discord
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("Discord adapter started (webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Discord adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        // If channel_id looks like a snowflake ID, use the REST API.
        // Otherwise, try the webhook URL.
        if channel_id.chars().all(|c| c.is_ascii_digit()) && !channel_id.is_empty() {
            self.api_send_message(channel_id, message).await
        } else if self.webhook_url.is_some() {
            self.webhook_send(message).await
        } else {
            Err(PunchError::Channel {
                channel: "discord".to_string(),
                message: "no valid channel_id or webhook_url to send to".to_string(),
            })
        }
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

    async fn validate_credentials(&self) -> PunchResult<()> {
        let resp = self
            .client
            .get("https://discord.com/api/v10/users/@me")
            .header("Authorization", format!("Bot {}", self.bot_token))
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "discord".to_string(),
                message: format!("credential validation failed: {}", e),
            })?;
        if !resp.status().is_success() {
            return Err(PunchError::Channel {
                channel: "discord".to_string(),
                message: "invalid bot token".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discord_adapter_creation() {
        let adapter = DiscordAdapter::new(
            "test-token".to_string(),
            Some("https://discord.com/api/webhooks/123/abc".to_string()),
        );
        assert_eq!(adapter.name(), "discord");
        assert_eq!(adapter.platform(), ChannelPlatform::Discord);
    }

    #[test]
    fn test_parse_discord_webhook_basic() {
        let adapter = DiscordAdapter::new("token".to_string(), None);

        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "Hello agent!",
            "author": {
                "id": "user456",
                "username": "alice",
                "bot": false
            },
            "timestamp": "2024-01-01T00:00:00+00:00"
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Discord);
        assert_eq!(msg.user_id, "user456");
        assert_eq!(msg.display_name, "alice");
        assert_eq!(msg.channel_id, "ch1");
        assert_eq!(msg.text, "Hello agent!");
        assert!(!msg.is_group);
    }

    #[test]
    fn test_parse_discord_webhook_filters_bot() {
        let adapter = DiscordAdapter::new("token".to_string(), None);

        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "Bot message",
            "author": {
                "id": "bot123",
                "username": "somebot",
                "bot": true
            }
        });

        let msg = adapter.parse_webhook_payload(&payload);
        assert!(msg.is_none());
    }

    #[test]
    fn test_parse_discord_webhook_empty_content() {
        let adapter = DiscordAdapter::new("token".to_string(), None);

        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "",
            "author": {
                "id": "user1",
                "username": "alice"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload);
        assert!(msg.is_none());
    }

    #[test]
    fn test_parse_discord_webhook_guild_message() {
        let adapter = DiscordAdapter::new("token".to_string(), None);

        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "guild_id": "guild1",
            "content": "Group message",
            "author": {
                "id": "user1",
                "username": "alice"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert!(msg.is_group);
    }

    #[tokio::test]
    async fn test_discord_adapter_start_stop() {
        let adapter = DiscordAdapter::new("token".to_string(), None);

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        assert!(adapter.status().connected);

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }

    #[test]
    fn test_parse_discord_no_author() {
        let adapter = DiscordAdapter::new("token".to_string(), None);
        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "Hello"
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_discord_no_author_id() {
        let adapter = DiscordAdapter::new("token".to_string(), None);
        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "Hello",
            "author": { "username": "alice" }
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_discord_bot_false_explicitly() {
        let adapter = DiscordAdapter::new("token".to_string(), None);
        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "Non-bot",
            "author": { "id": "user1", "username": "alice", "bot": false }
        });
        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.text, "Non-bot");
    }

    #[test]
    fn test_parse_discord_no_content() {
        let adapter = DiscordAdapter::new("token".to_string(), None);
        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "author": { "id": "user1", "username": "alice" }
        });
        // content defaults to "" which is empty
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_discord_dm_no_guild() {
        let adapter = DiscordAdapter::new("token".to_string(), None);
        let payload = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "DM message",
            "author": { "id": "user1", "username": "alice" }
        });
        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert!(!msg.is_group);
    }

    #[test]
    fn test_parse_discord_message_counter() {
        let adapter = DiscordAdapter::new("token".to_string(), None);
        assert_eq!(adapter.status().messages_received, 0);
        let payload = serde_json::json!({
            "id": "msg1", "channel_id": "ch1", "content": "test",
            "author": { "id": "u1", "username": "a" }
        });
        adapter.parse_webhook_payload(&payload);
        assert_eq!(adapter.status().messages_received, 1);
    }
}
