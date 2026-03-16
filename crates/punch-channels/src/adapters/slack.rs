//! Slack channel adapter (webhook-based).
//!
//! Receives messages via Slack Events API (POST /api/channels/slack/events)
//! and sends responses back via Slack Web API (chat.postMessage).
//! Also handles URL verification challenges.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

type HmacSha256 = Hmac<Sha256>;

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage, split_message};

const SLACK_MSG_LIMIT: usize = 3000;

/// Slack Events API adapter.
///
/// Receives: Slack Events API payloads via POST to the Arena endpoint.
/// Sends: responses via Slack Web API `chat.postMessage`.
pub struct SlackAdapter {
    /// Bot token for the Slack Web API (xoxb-...).
    bot_token: String,
    /// Signing secret for verifying Slack requests.
    signing_secret: Option<String>,
    /// HTTP client for API calls.
    client: reqwest::Client,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
    /// Bot's own user ID (to filter out self-messages).
    bot_user_id: RwLock<Option<String>>,
}

impl SlackAdapter {
    /// Create a new Slack adapter.
    ///
    /// `bot_token`: Slack bot token (xoxb-..., read from env by caller).
    /// `signing_secret`: Optional Slack signing secret for request verification.
    pub fn new(bot_token: String, signing_secret: Option<String>) -> Self {
        Self {
            bot_token,
            signing_secret,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
            bot_user_id: RwLock::new(None),
        }
    }

    /// Check if this is a URL verification challenge from Slack.
    ///
    /// Returns Some(challenge_value) if this is a challenge, None otherwise.
    pub fn check_url_verification(&self, payload: &serde_json::Value) -> Option<String> {
        if payload["type"].as_str() == Some("url_verification") {
            payload["challenge"].as_str().map(String::from)
        } else {
            None
        }
    }

    /// Verify a Slack webhook request signature.
    ///
    /// Slack signs every webhook with HMAC-SHA256 using the signing secret.
    /// The signature is computed over `v0:{timestamp}:{body}` and compared
    /// in constant time against the `X-Slack-Signature` header value.
    ///
    /// Returns `true` if the signature is valid, `false` if verification
    /// fails or no signing secret is configured.
    pub fn verify_webhook_signature(
        &self,
        timestamp: &str,
        signature: &str,
        body: &[u8],
    ) -> bool {
        let secret = match &self.signing_secret {
            Some(s) => s,
            None => return false,
        };

        let basestring = format!("v0:{}:{}", timestamp, String::from_utf8_lossy(body));

        let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(basestring.as_bytes());
        let expected = mac.finalize().into_bytes();
        let expected_hex = format!("v0={}", hex_encode(&expected));

        constant_time_eq(expected_hex.as_bytes(), signature.as_bytes())
    }

    /// Parse a Slack Events API payload into an IncomingMessage.
    ///
    /// Expected JSON format:
    /// ```json
    /// {
    ///   "type": "event_callback",
    ///   "event": {
    ///     "type": "message",
    ///     "user": "U456",
    ///     "channel": "C789",
    ///     "text": "Hello agent!",
    ///     "ts": "1700000000.000100"
    ///   }
    /// }
    /// ```
    pub async fn parse_webhook_payload(
        &self,
        payload: &serde_json::Value,
    ) -> Option<IncomingMessage> {
        let payload_type = payload["type"].as_str()?;
        if payload_type != "event_callback" {
            return None;
        }

        let event = payload.get("event")?;
        let event_type = event["type"].as_str()?;

        if event_type != "message" {
            return None;
        }

        // Skip subtypes (joins, leaves, bot messages, etc.) except message_changed
        let subtype = event["subtype"].as_str();
        let (msg_data, _is_edit) = match subtype {
            Some("message_changed") => match event.get("message") {
                Some(inner) => (inner, true),
                None => return None,
            },
            Some(_) => return None,
            None => (event, false),
        };

        // Filter out bot messages
        if msg_data.get("bot_id").is_some() {
            return None;
        }

        let user_id = msg_data["user"]
            .as_str()
            .or_else(|| event["user"].as_str())?;

        // Filter out own messages
        if let Some(ref bid) = *self.bot_user_id.read().await
            && user_id == bid
        {
            return None;
        }

        let channel = event["channel"].as_str()?;
        let text = msg_data["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }

        let ts = msg_data["ts"]
            .as_str()
            .unwrap_or(event["ts"].as_str().unwrap_or("0"));

        let timestamp = ts
            .split('.')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|epoch| DateTime::from_timestamp(epoch, 0))
            .unwrap_or_else(Utc::now);

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: channel.to_string(),
            user_id: user_id.to_string(),
            display_name: user_id.to_string(), // Slack doesn't include display name in events
            text: text.to_string(),
            timestamp,
            platform: ChannelPlatform::Slack,
            platform_message_id: ts.to_string(),
            is_group: true, // Slack channels are inherently group-like
            metadata: std::collections::HashMap::new(),
        })
    }

    /// Send a message via Slack Web API chat.postMessage.
    async fn api_send_message(&self, channel_id: &str, text: &str) -> PunchResult<()> {
        let chunks = split_message(text, SLACK_MSG_LIMIT);

        for chunk in chunks {
            let body = serde_json::json!({
                "channel": channel_id,
                "text": chunk,
            });

            let resp: serde_json::Value = self
                .client
                .post("https://slack.com/api/chat.postMessage")
                .header("Authorization", format!("Bearer {}", self.bot_token))
                .json(&body)
                .send()
                .await
                .map_err(|e| PunchError::Channel {
                    channel: "slack".to_string(),
                    message: format!("failed to send message: {e}"),
                })?
                .json()
                .await
                .map_err(|e| PunchError::Channel {
                    channel: "slack".to_string(),
                    message: format!("failed to parse response: {e}"),
                })?;

            if resp["ok"].as_bool() != Some(true) {
                let err = resp["error"].as_str().unwrap_or("unknown");
                warn!("Slack chat.postMessage failed: {err}");
            }
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Set the bot's own user ID (for filtering self-messages).
    pub async fn set_bot_user_id(&self, user_id: String) {
        *self.bot_user_id.write().await = Some(user_id);
    }
}

#[async_trait]
impl ChannelAdapter for SlackAdapter {
    fn name(&self) -> &str {
        "slack"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Slack
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("Slack adapter started (Events API webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Slack adapter stopped");
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
        let resp = self
            .client
            .get("https://slack.com/api/auth.test")
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "slack".to_string(),
                message: format!("credential validation failed: {}", e),
            })?;
        if !resp.status().is_success() {
            return Err(PunchError::Channel {
                channel: "slack".to_string(),
                message: "invalid bot token".to_string(),
            });
        }
        Ok(())
    }
}

/// Encode bytes as lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slack_adapter_creation() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), Some("secret".to_string()));
        assert_eq!(adapter.name(), "slack");
        assert_eq!(adapter.platform(), ChannelPlatform::Slack);
    }

    #[test]
    fn test_check_url_verification() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);

        let challenge = serde_json::json!({
            "type": "url_verification",
            "challenge": "test_challenge_value"
        });

        let result = adapter.check_url_verification(&challenge);
        assert_eq!(result, Some("test_challenge_value".to_string()));

        let non_challenge = serde_json::json!({
            "type": "event_callback",
            "event": {}
        });

        let result = adapter.check_url_verification(&non_challenge);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_parse_slack_event_basic() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);

        let payload = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "user": "U456",
                "channel": "C789",
                "text": "Hello agent!",
                "ts": "1700000000.000100"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).await.unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Slack);
        assert_eq!(msg.user_id, "U456");
        assert_eq!(msg.channel_id, "C789");
        assert_eq!(msg.text, "Hello agent!");
    }

    #[tokio::test]
    async fn test_parse_slack_event_filters_bot() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);

        let payload = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "user": "U456",
                "channel": "C789",
                "text": "Bot message",
                "ts": "1700000000.000100",
                "bot_id": "B999"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).await;
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_parse_slack_event_skips_subtypes() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);

        let payload = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "subtype": "channel_join",
                "user": "U456",
                "channel": "C789",
                "text": "joined",
                "ts": "1700000000.000100"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).await;
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_parse_slack_event_message_changed() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);

        let payload = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "subtype": "message_changed",
                "channel": "C789",
                "message": {
                    "user": "U456",
                    "text": "Edited text",
                    "ts": "1700000000.000100"
                },
                "ts": "1700000001.000200"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).await.unwrap();
        assert_eq!(msg.text, "Edited text");
    }

    #[tokio::test]
    async fn test_slack_adapter_start_stop() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        assert!(adapter.status().connected);

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }

    #[tokio::test]
    async fn test_parse_slack_event_empty_text() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);
        let payload = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "user": "U1",
                "channel": "C1",
                "text": "",
                "ts": "1700000000.000100"
            }
        });
        assert!(adapter.parse_webhook_payload(&payload).await.is_none());
    }

    #[tokio::test]
    async fn test_parse_slack_event_wrong_type() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);
        let payload = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "reaction_added",
                "user": "U1",
                "channel": "C1",
                "ts": "1700000000.000100"
            }
        });
        assert!(adapter.parse_webhook_payload(&payload).await.is_none());
    }

    #[tokio::test]
    async fn test_parse_slack_filters_own_bot_messages() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);
        adapter.set_bot_user_id("UBOTSELF".to_string()).await;

        let payload = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "user": "UBOTSELF",
                "channel": "C1",
                "text": "My own msg",
                "ts": "1700000000.000100"
            }
        });
        assert!(adapter.parse_webhook_payload(&payload).await.is_none());
    }

    #[tokio::test]
    async fn test_parse_slack_subtype_channel_leave() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);
        let payload = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "subtype": "channel_leave",
                "user": "U1",
                "channel": "C1",
                "text": "left",
                "ts": "1700000000.000100"
            }
        });
        assert!(adapter.parse_webhook_payload(&payload).await.is_none());
    }

    #[test]
    fn test_check_url_verification_no_challenge() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);
        let payload = serde_json::json!({ "type": "url_verification" });
        // No challenge field
        assert!(adapter.check_url_verification(&payload).is_none());
    }

    #[tokio::test]
    async fn test_parse_slack_not_event_callback() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);
        let payload = serde_json::json!({
            "type": "url_verification",
            "challenge": "abc"
        });
        assert!(adapter.parse_webhook_payload(&payload).await.is_none());
    }

    // --- Webhook signature verification tests ---

    fn make_slack_signature(secret: &str, timestamp: &str, body: &[u8]) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;

        let basestring = format!("v0:{}:{}", timestamp, String::from_utf8_lossy(body));
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(basestring.as_bytes());
        let result = mac.finalize().into_bytes();
        let hex: String = result.iter().map(|b| format!("{:02x}", b)).collect();
        format!("v0={}", hex)
    }

    #[test]
    fn test_verify_webhook_signature_valid() {
        let secret = "test_signing_secret_12345";
        let adapter = SlackAdapter::new("xoxb-test".to_string(), Some(secret.to_string()));

        let timestamp = "1700000000";
        let body = b"{\"type\":\"event_callback\",\"event\":{}}";
        let signature = make_slack_signature(secret, timestamp, body);

        assert!(adapter.verify_webhook_signature(timestamp, &signature, body));
    }

    #[test]
    fn test_verify_webhook_signature_invalid() {
        let secret = "test_signing_secret_12345";
        let adapter = SlackAdapter::new("xoxb-test".to_string(), Some(secret.to_string()));

        let timestamp = "1700000000";
        let body = b"{\"type\":\"event_callback\",\"event\":{}}";

        assert!(!adapter.verify_webhook_signature(timestamp, "v0=deadbeef", body));
    }

    #[test]
    fn test_verify_webhook_signature_no_secret() {
        let adapter = SlackAdapter::new("xoxb-test".to_string(), None);

        assert!(!adapter.verify_webhook_signature("1700000000", "v0=abc", b"body"));
    }

    #[test]
    fn test_verify_webhook_signature_tampered_body() {
        let secret = "my_secret";
        let adapter = SlackAdapter::new("xoxb-test".to_string(), Some(secret.to_string()));

        let timestamp = "1700000000";
        let original_body = b"original body";
        let signature = make_slack_signature(secret, timestamp, original_body);

        // Tampered body should fail
        assert!(!adapter.verify_webhook_signature(timestamp, &signature, b"tampered body"));
    }

    #[test]
    fn test_verify_webhook_signature_tampered_timestamp() {
        let secret = "my_secret";
        let adapter = SlackAdapter::new("xoxb-test".to_string(), Some(secret.to_string()));

        let body = b"test body";
        let signature = make_slack_signature(secret, "1700000000", body);

        // Different timestamp should fail
        assert!(!adapter.verify_webhook_signature("1700000001", &signature, body));
    }

    #[test]
    fn test_verify_webhook_signature_empty_body() {
        let secret = "secret";
        let adapter = SlackAdapter::new("xoxb-test".to_string(), Some(secret.to_string()));

        let timestamp = "1700000000";
        let body = b"";
        let signature = make_slack_signature(secret, timestamp, body);

        assert!(adapter.verify_webhook_signature(timestamp, &signature, body));
    }
}
