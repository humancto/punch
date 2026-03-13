//! Telegram channel adapter (webhook-based).
//!
//! Receives messages via a webhook endpoint (POST /api/channels/telegram/webhook)
//! and sends responses back via the Telegram Bot API (sendMessage).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage, split_message};

const TELEGRAM_MSG_LIMIT: usize = 4096;

/// Telegram Bot API adapter using webhooks.
///
/// Receives: Telegram Update JSON via POST to the Arena webhook endpoint.
/// Sends: responses via Telegram Bot API `sendMessage`.
pub struct TelegramAdapter {
    /// Bot token for the Telegram Bot API.
    bot_token: String,
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

impl TelegramAdapter {
    /// Create a new Telegram adapter.
    ///
    /// `bot_token`: The Telegram bot token (read from env by caller).
    pub fn new(bot_token: String) -> Self {
        Self {
            bot_token,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Parse a Telegram Update JSON into an IncomingMessage.
    ///
    /// Expected JSON format (Telegram Update):
    /// ```json
    /// {
    ///   "update_id": 123456,
    ///   "message": {
    ///     "message_id": 42,
    ///     "from": { "id": 111222333, "first_name": "Alice", "last_name": "Smith" },
    ///     "chat": { "id": 111222333, "type": "private" },
    ///     "date": 1700000000,
    ///     "text": "Hello, agent!"
    ///   }
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let message = payload
            .get("message")
            .or_else(|| payload.get("edited_message"))?;

        let from = message.get("from")?;
        let user_id = from["id"].as_i64()?;
        let chat_id = message["chat"]["id"].as_i64()?;
        let message_id = message["message_id"].as_i64().unwrap_or(0);

        let first_name = from["first_name"].as_str().unwrap_or("Unknown");
        let last_name = from["last_name"].as_str().unwrap_or("");
        let display_name = if last_name.is_empty() {
            first_name.to_string()
        } else {
            format!("{first_name} {last_name}")
        };

        let text = message["text"].as_str()?;
        if text.is_empty() {
            return None;
        }

        let chat_type = message["chat"]["type"].as_str().unwrap_or("private");
        let is_group = chat_type == "group" || chat_type == "supergroup";

        let timestamp = message["date"]
            .as_i64()
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
            .unwrap_or_else(Utc::now);

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: chat_id.to_string(),
            user_id: user_id.to_string(),
            display_name,
            text: text.to_string(),
            timestamp,
            platform: ChannelPlatform::Telegram,
            platform_message_id: message_id.to_string(),
            is_group,
            metadata: std::collections::HashMap::new(),
        })
    }

    /// Send a message via the Telegram Bot API.
    async fn api_send_message(&self, chat_id: &str, text: &str) -> PunchResult<()> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);

        let chunks = split_message(text, TELEGRAM_MSG_LIMIT);
        for chunk in chunks {
            let body = serde_json::json!({
                "chat_id": chat_id,
                "text": chunk,
            });

            let resp = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| PunchError::Channel {
                    channel: "telegram".to_string(),
                    message: format!("failed to send message: {e}"),
                })?;

            let status = resp.status();
            if !status.is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                warn!("Telegram sendMessage failed ({status}): {body_text}");
            }
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for TelegramAdapter {
    fn name(&self) -> &str {
        "telegram"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Telegram
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("Telegram adapter started (webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Telegram adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.api_send_message(channel_id, message).await
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

    #[test]
    fn test_telegram_adapter_creation() {
        let adapter = TelegramAdapter::new("test-token".to_string());
        assert_eq!(adapter.name(), "telegram");
        assert_eq!(adapter.platform(), ChannelPlatform::Telegram);
    }

    #[test]
    fn test_parse_telegram_update_basic() {
        let adapter = TelegramAdapter::new("token".to_string());

        let payload = serde_json::json!({
            "update_id": 123456,
            "message": {
                "message_id": 42,
                "from": {
                    "id": 111222333,
                    "first_name": "Alice",
                    "last_name": "Smith"
                },
                "chat": {
                    "id": 111222333,
                    "type": "private"
                },
                "date": 1700000000,
                "text": "Hello, agent!"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Telegram);
        assert_eq!(msg.user_id, "111222333");
        assert_eq!(msg.display_name, "Alice Smith");
        assert_eq!(msg.channel_id, "111222333");
        assert_eq!(msg.text, "Hello, agent!");
        assert!(!msg.is_group);
    }

    #[test]
    fn test_parse_telegram_group_message() {
        let adapter = TelegramAdapter::new("token".to_string());

        let payload = serde_json::json!({
            "update_id": 123457,
            "message": {
                "message_id": 43,
                "from": {
                    "id": 111,
                    "first_name": "Bob"
                },
                "chat": {
                    "id": -1001234567890i64,
                    "type": "supergroup"
                },
                "date": 1700000001,
                "text": "Group message"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert!(msg.is_group);
        assert_eq!(msg.channel_id, "-1001234567890");
    }

    #[test]
    fn test_parse_telegram_edited_message() {
        let adapter = TelegramAdapter::new("token".to_string());

        let payload = serde_json::json!({
            "update_id": 123459,
            "edited_message": {
                "message_id": 42,
                "from": {
                    "id": 111,
                    "first_name": "Alice"
                },
                "chat": {
                    "id": 111,
                    "type": "private"
                },
                "date": 1700000000,
                "text": "Edited message"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.text, "Edited message");
    }

    #[test]
    fn test_parse_telegram_no_text() {
        let adapter = TelegramAdapter::new("token".to_string());

        // Sticker message (no text field)
        let payload = serde_json::json!({
            "update_id": 123460,
            "message": {
                "message_id": 50,
                "from": { "id": 111, "first_name": "Alice" },
                "chat": { "id": 111, "type": "private" },
                "date": 1700000000,
                "sticker": { "file_id": "abc" }
            }
        });

        let msg = adapter.parse_webhook_payload(&payload);
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_telegram_adapter_start_stop() {
        let adapter = TelegramAdapter::new("token".to_string());

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        assert!(adapter.status().connected);

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
