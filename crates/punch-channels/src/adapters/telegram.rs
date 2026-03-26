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

/// Metadata key for base64-encoded image data from a Telegram photo.
pub const META_IMAGE_BASE64: &str = "image_base64";
/// Metadata key for the image MIME type.
pub const META_IMAGE_MEDIA_TYPE: &str = "image_media_type";

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

    /// Parse a Telegram Update, including photo messages.
    ///
    /// For photo messages, downloads the largest photo via the Bot API,
    /// base64-encodes it, and stores it in metadata as `image_base64` and
    /// `image_media_type`. Caption (if any) becomes the text; if there is
    /// no caption, text is left empty and the image content speaks for itself.
    pub async fn parse_webhook_payload_with_photos(
        &self,
        payload: &serde_json::Value,
    ) -> Option<IncomingMessage> {
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

        let chat_type = message["chat"]["type"].as_str().unwrap_or("private");
        let is_group = chat_type == "group" || chat_type == "supergroup";

        let timestamp = message["date"]
            .as_i64()
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
            .unwrap_or_else(Utc::now);

        // Check for photo array (Telegram sends multiple sizes, largest is last).
        let mut metadata = std::collections::HashMap::new();
        let has_photo = if let Some(photos) = message.get("photo").and_then(|p| p.as_array()) {
            if let Some(largest) = photos.last() {
                if let Some(file_id) = largest["file_id"].as_str() {
                    match self.download_file(file_id).await {
                        Ok(data) => {
                            use base64::Engine;
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                            metadata.insert(
                                META_IMAGE_BASE64.to_string(),
                                serde_json::Value::String(b64),
                            );
                            metadata.insert(
                                META_IMAGE_MEDIA_TYPE.to_string(),
                                serde_json::Value::String("image/jpeg".to_string()),
                            );
                            true
                        }
                        Err(e) => {
                            warn!("Failed to download Telegram photo: {e}");
                            false
                        }
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        // Text: use actual text or caption. For photo-only messages, leave empty
        // and let the LLM's multimodal training handle it naturally.
        let text = message["text"]
            .as_str()
            .or_else(|| message["caption"].as_str())
            .unwrap_or("");

        if text.is_empty() && !has_photo {
            return None;
        }

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
            metadata,
        })
    }

    /// Download a file from Telegram by file_id.
    ///
    /// Two-step process:
    /// 1. `getFile` to get the file path
    /// 2. Download the file bytes
    async fn download_file(&self, file_id: &str) -> PunchResult<Vec<u8>> {
        // Step 1: Get file path.
        let get_file_url = format!(
            "https://api.telegram.org/bot{}/getFile?file_id={}",
            self.bot_token, file_id
        );
        let resp =
            self.client
                .get(&get_file_url)
                .send()
                .await
                .map_err(|e| PunchError::Channel {
                    channel: "telegram".to_string(),
                    message: format!("getFile failed: {e}"),
                })?;

        let body: serde_json::Value = resp.json().await.map_err(|e| PunchError::Channel {
            channel: "telegram".to_string(),
            message: format!("getFile parse failed: {e}"),
        })?;

        let file_path =
            body["result"]["file_path"]
                .as_str()
                .ok_or_else(|| PunchError::Channel {
                    channel: "telegram".to_string(),
                    message: "getFile response missing file_path".to_string(),
                })?;

        // Step 2: Download file bytes.
        let download_url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.bot_token, file_path
        );
        let file_resp =
            self.client
                .get(&download_url)
                .send()
                .await
                .map_err(|e| PunchError::Channel {
                    channel: "telegram".to_string(),
                    message: format!("file download failed: {e}"),
                })?;

        let bytes = file_resp.bytes().await.map_err(|e| PunchError::Channel {
            channel: "telegram".to_string(),
            message: format!("file read failed: {e}"),
        })?;

        Ok(bytes.to_vec())
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

    async fn validate_credentials(&self) -> PunchResult<()> {
        let url = format!("https://api.telegram.org/bot{}/getMe", self.bot_token);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "telegram".to_string(),
                message: format!("credential validation failed: {}", e),
            })?;
        if !resp.status().is_success() {
            return Err(PunchError::Channel {
                channel: "telegram".to_string(),
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

    #[test]
    fn test_parse_telegram_empty_text() {
        let adapter = TelegramAdapter::new("token".to_string());
        let payload = serde_json::json!({
            "update_id": 123,
            "message": {
                "message_id": 1,
                "from": { "id": 111, "first_name": "Alice" },
                "chat": { "id": 111, "type": "private" },
                "date": 1700000000,
                "text": ""
            }
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_telegram_no_from() {
        let adapter = TelegramAdapter::new("token".to_string());
        let payload = serde_json::json!({
            "update_id": 123,
            "message": {
                "message_id": 1,
                "chat": { "id": 111, "type": "private" },
                "date": 1700000000,
                "text": "Hello"
            }
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_telegram_first_name_only() {
        let adapter = TelegramAdapter::new("token".to_string());
        let payload = serde_json::json!({
            "update_id": 123,
            "message": {
                "message_id": 1,
                "from": { "id": 111, "first_name": "Alice" },
                "chat": { "id": 111, "type": "private" },
                "date": 1700000000,
                "text": "Hi"
            }
        });
        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.display_name, "Alice");
    }

    #[test]
    fn test_parse_telegram_group_type() {
        let adapter = TelegramAdapter::new("token".to_string());
        let payload = serde_json::json!({
            "update_id": 123,
            "message": {
                "message_id": 1,
                "from": { "id": 111, "first_name": "Bob" },
                "chat": { "id": -100, "type": "group" },
                "date": 1700000000,
                "text": "Group msg"
            }
        });
        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert!(msg.is_group);
    }

    #[test]
    fn test_parse_telegram_message_counter() {
        let adapter = TelegramAdapter::new("token".to_string());
        assert_eq!(adapter.status().messages_received, 0);

        let payload = serde_json::json!({
            "update_id": 123,
            "message": {
                "message_id": 1,
                "from": { "id": 111, "first_name": "Alice" },
                "chat": { "id": 111, "type": "private" },
                "date": 1700000000,
                "text": "Test"
            }
        });
        adapter.parse_webhook_payload(&payload);
        assert_eq!(adapter.status().messages_received, 1);

        adapter.parse_webhook_payload(&payload);
        assert_eq!(adapter.status().messages_received, 2);
    }

    #[test]
    fn test_parse_telegram_no_message_key() {
        let adapter = TelegramAdapter::new("token".to_string());
        let payload = serde_json::json!({ "update_id": 123 });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_sync_skips_photo_only_messages() {
        // The sync parser requires text — photo-only messages need the async parser.
        let adapter = TelegramAdapter::new("token".to_string());
        let payload = serde_json::json!({
            "update_id": 123,
            "message": {
                "message_id": 1,
                "from": { "id": 111, "first_name": "Alice" },
                "chat": { "id": 111, "type": "private" },
                "date": 1700000000,
                "photo": [
                    { "file_id": "small_id", "width": 90, "height": 90 },
                    { "file_id": "large_id", "width": 800, "height": 600 }
                ]
            }
        });
        // No text field → sync parser returns None (photo needs async parser).
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_meta_keys_are_constant() {
        assert_eq!(META_IMAGE_BASE64, "image_base64");
        assert_eq!(META_IMAGE_MEDIA_TYPE, "image_media_type");
    }
}
