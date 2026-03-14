//! WhatsApp Business API channel adapter (webhook-based).
//!
//! Receives messages via WhatsApp Cloud API webhooks and sends responses
//! back via the WhatsApp Cloud API messages endpoint.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const WHATSAPP_API_BASE: &str = "https://graph.facebook.com/v21.0";

/// WhatsApp Business Cloud API adapter.
///
/// Receives: WhatsApp webhook payloads via POST to the Arena webhook endpoint.
/// Sends: responses via the WhatsApp Cloud API messages endpoint.
pub struct WhatsAppAdapter {
    /// API access token for the WhatsApp Business Cloud API.
    api_token: String,
    /// Phone number ID for sending messages.
    phone_number_id: String,
    /// Webhook verify token for incoming webhook validation.
    #[allow(dead_code)]
    webhook_verify_token: String,
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

impl WhatsAppAdapter {
    /// Create a new WhatsApp adapter.
    ///
    /// `api_token`: WhatsApp Cloud API access token.
    /// `phone_number_id`: The phone number ID for sending messages.
    /// `webhook_verify_token`: Token used to verify incoming webhook subscriptions.
    pub fn new(api_token: String, phone_number_id: String, webhook_verify_token: String) -> Self {
        Self {
            api_token,
            phone_number_id,
            webhook_verify_token,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Parse a WhatsApp Cloud API webhook payload into an `IncomingMessage`.
    ///
    /// Expected JSON format:
    /// ```json
    /// {
    ///   "object": "whatsapp_business_account",
    ///   "entry": [{
    ///     "id": "BIZ_ACCOUNT_ID",
    ///     "changes": [{
    ///       "value": {
    ///         "messaging_product": "whatsapp",
    ///         "metadata": { "phone_number_id": "123", "display_phone_number": "+1..." },
    ///         "contacts": [{ "profile": { "name": "Alice" }, "wa_id": "15551234567" }],
    ///         "messages": [{
    ///           "from": "15551234567",
    ///           "id": "wamid.abc123",
    ///           "timestamp": "1700000000",
    ///           "type": "text",
    ///           "text": { "body": "Hello!" }
    ///         }]
    ///       },
    ///       "field": "messages"
    ///     }]
    ///   }]
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let entry = payload.get("entry")?.as_array()?.first()?;
        let changes = entry.get("changes")?.as_array()?.first()?;
        let value = changes.get("value")?;

        // Only process message webhooks
        let field = changes["field"].as_str()?;
        if field != "messages" {
            return None;
        }

        let messages = value.get("messages")?.as_array()?;
        let message = messages.first()?;

        // Only handle text messages
        let msg_type = message["type"].as_str()?;
        if msg_type != "text" {
            return None;
        }

        let from = message["from"].as_str()?;
        let message_id = message["id"].as_str().unwrap_or("unknown");
        let text = message["text"]["body"].as_str()?;
        if text.is_empty() {
            return None;
        }

        // Extract contact display name
        let display_name = value
            .get("contacts")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c["profile"]["name"].as_str())
            .unwrap_or(from);

        let timestamp = message["timestamp"]
            .as_str()
            .and_then(|ts| ts.parse::<i64>().ok())
            .and_then(|epoch| DateTime::from_timestamp(epoch, 0))
            .unwrap_or_else(Utc::now);

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: from.to_string(),
            user_id: from.to_string(),
            display_name: display_name.to_string(),
            text: text.to_string(),
            timestamp,
            platform: ChannelPlatform::WhatsApp,
            platform_message_id: message_id.to_string(),
            is_group: false,
            metadata: HashMap::new(),
        })
    }

    /// Send a text message via the WhatsApp Cloud API.
    async fn api_send_message(&self, recipient_phone: &str, text: &str) -> PunchResult<()> {
        let url = format!("{}/{}/messages", WHATSAPP_API_BASE, self.phone_number_id);

        let body = serde_json::json!({
            "messaging_product": "whatsapp",
            "recipient_type": "individual",
            "to": recipient_phone,
            "type": "text",
            "text": {
                "preview_url": false,
                "body": text
            }
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "whatsapp".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("WhatsApp send message failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for WhatsAppAdapter {
    fn name(&self) -> &str {
        "whatsapp"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::WhatsApp
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("WhatsApp adapter started (webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("WhatsApp adapter stopped");
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
        let url = format!("{}/{}", WHATSAPP_API_BASE, self.phone_number_id);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "whatsapp".to_string(),
                message: format!("credential validation failed: {}", e),
            })?;
        if !resp.status().is_success() {
            return Err(PunchError::Channel {
                channel: "whatsapp".to_string(),
                message: "invalid credentials".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_adapter() -> WhatsAppAdapter {
        WhatsAppAdapter::new(
            "test-token".to_string(),
            "123456789".to_string(),
            "verify-token".to_string(),
        )
    }

    #[test]
    fn test_whatsapp_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "whatsapp");
        assert_eq!(adapter.platform(), ChannelPlatform::WhatsApp);
    }

    #[test]
    fn test_parse_whatsapp_webhook_basic() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "BIZ_123",
                "changes": [{
                    "value": {
                        "messaging_product": "whatsapp",
                        "metadata": {
                            "phone_number_id": "123456789",
                            "display_phone_number": "+15551234567"
                        },
                        "contacts": [{
                            "profile": { "name": "Alice" },
                            "wa_id": "15559876543"
                        }],
                        "messages": [{
                            "from": "15559876543",
                            "id": "wamid.abc123",
                            "timestamp": "1700000000",
                            "type": "text",
                            "text": { "body": "Hello from WhatsApp!" }
                        }]
                    },
                    "field": "messages"
                }]
            }]
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::WhatsApp);
        assert_eq!(msg.user_id, "15559876543");
        assert_eq!(msg.display_name, "Alice");
        assert_eq!(msg.text, "Hello from WhatsApp!");
        assert_eq!(msg.platform_message_id, "wamid.abc123");
        assert!(!msg.is_group);
    }

    #[test]
    fn test_parse_whatsapp_webhook_non_text() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "BIZ_123",
                "changes": [{
                    "value": {
                        "messaging_product": "whatsapp",
                        "contacts": [{ "profile": { "name": "Bob" }, "wa_id": "15551111111" }],
                        "messages": [{
                            "from": "15551111111",
                            "id": "wamid.xyz",
                            "timestamp": "1700000000",
                            "type": "image",
                            "image": { "id": "img123" }
                        }]
                    },
                    "field": "messages"
                }]
            }]
        });

        let msg = adapter.parse_webhook_payload(&payload);
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_whatsapp_adapter_start_stop() {
        let adapter = make_adapter();

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        let status = adapter.status();
        assert!(status.connected);
        assert!(status.started_at.is_some());

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }

    #[test]
    fn test_parse_whatsapp_empty_text() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{"id": "B", "changes": [{"value": {
                "contacts": [{"profile": {"name": "A"}, "wa_id": "1"}],
                "messages": [{"from": "1", "id": "w1", "timestamp": "1700000000",
                    "type": "text", "text": {"body": ""}}]
            }, "field": "messages"}]}]
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_whatsapp_no_contacts() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{"id": "B", "changes": [{"value": {
                "messages": [{"from": "1234", "id": "w1", "timestamp": "1700000000",
                    "type": "text", "text": {"body": "Hi"}}]
            }, "field": "messages"}]}]
        });
        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        // display_name falls back to phone number
        assert_eq!(msg.display_name, "1234");
    }

    #[test]
    fn test_parse_whatsapp_status_field_ignored() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{"id": "B", "changes": [{"value": {
                "statuses": [{"id": "s1"}]
            }, "field": "statuses"}]}]
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_whatsapp_video_type_ignored() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{"id": "B", "changes": [{"value": {
                "contacts": [{"profile": {"name": "A"}, "wa_id": "1"}],
                "messages": [{"from": "1", "id": "w1", "timestamp": "1700000000",
                    "type": "video", "video": {"id": "v1"}}]
            }, "field": "messages"}]}]
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_parse_whatsapp_empty_entry() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": []
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }
}
