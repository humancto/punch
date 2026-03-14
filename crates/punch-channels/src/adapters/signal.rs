//! Signal messenger adapter via Signal Bot API.
//!
//! Receives messages via Signal REST API webhooks and sends responses
//! back via HTTP POST to the Signal REST API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// Signal messenger adapter using the signal-cli REST API.
///
/// Receives: Signal webhook payloads via POST to the Arena webhook endpoint.
/// Sends: responses via HTTP POST to the Signal REST API.
pub struct SignalAdapter {
    /// The phone number registered with Signal (e.g. "+15551234567").
    phone_number: String,
    /// Base URL for the Signal REST API (e.g. "http://localhost:8080").
    api_url: String,
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

impl SignalAdapter {
    /// Create a new Signal adapter.
    ///
    /// `phone_number`: The phone number registered with Signal.
    /// `api_url`: Base URL for the signal-cli REST API.
    pub fn new(phone_number: String, api_url: String) -> Self {
        Self {
            phone_number,
            api_url,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Parse a Signal REST API webhook payload into an `IncomingMessage`.
    ///
    /// Expected JSON format:
    /// ```json
    /// {
    ///   "envelope": {
    ///     "source": "+15559876543",
    ///     "sourceName": "Alice",
    ///     "timestamp": 1700000000000,
    ///     "dataMessage": {
    ///       "message": "Hello from Signal!",
    ///       "timestamp": 1700000000000,
    ///       "groupInfo": null
    ///     }
    ///   }
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let envelope = payload.get("envelope")?;
        let source = envelope.get("source")?.as_str()?;
        let source_name = envelope
            .get("sourceName")
            .and_then(|v| v.as_str())
            .unwrap_or(source);

        let data_message = envelope.get("dataMessage")?;
        let text = data_message.get("message")?.as_str()?;
        if text.is_empty() {
            return None;
        }

        let timestamp_ms = envelope
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let timestamp = DateTime::from_timestamp(timestamp_ms / 1000, 0).unwrap_or_else(Utc::now);

        let is_group = data_message.get("groupInfo").is_some_and(|v| !v.is_null());

        let channel_id = if is_group {
            data_message
                .get("groupInfo")
                .and_then(|g| g.get("groupId"))
                .and_then(|v| v.as_str())
                .unwrap_or(source)
                .to_string()
        } else {
            source.to_string()
        };

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id,
            user_id: source.to_string(),
            display_name: source_name.to_string(),
            text: text.to_string(),
            timestamp,
            platform: ChannelPlatform::Signal,
            platform_message_id: timestamp_ms.to_string(),
            is_group,
            metadata: HashMap::new(),
        })
    }

    /// Send a text message via the Signal REST API.
    async fn api_send_message(&self, recipient: &str, text: &str) -> PunchResult<()> {
        let url = format!("{}/v2/send", self.api_url);

        let body = serde_json::json!({
            "message": text,
            "number": self.phone_number,
            "recipients": [recipient],
        });

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "signal".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Signal send message failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for SignalAdapter {
    fn name(&self) -> &str {
        "signal"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Signal
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("Signal adapter started (webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Signal adapter stopped");
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

    fn make_adapter() -> SignalAdapter {
        SignalAdapter::new(
            "+15551234567".to_string(),
            "http://localhost:8080".to_string(),
        )
    }

    #[test]
    fn test_signal_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "signal");
        assert_eq!(adapter.platform(), ChannelPlatform::Signal);
    }

    #[test]
    fn test_parse_signal_webhook_basic() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "envelope": {
                "source": "+15559876543",
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "Hello from Signal!",
                    "timestamp": 1700000000000_i64,
                    "groupInfo": null
                }
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Signal);
        assert_eq!(msg.user_id, "+15559876543");
        assert_eq!(msg.display_name, "Alice");
        assert_eq!(msg.text, "Hello from Signal!");
        assert!(!msg.is_group);
    }

    #[test]
    fn test_parse_signal_webhook_group_message() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "envelope": {
                "source": "+15559876543",
                "sourceName": "Bob",
                "timestamp": 1700000001000_i64,
                "dataMessage": {
                    "message": "Group hello!",
                    "timestamp": 1700000001000_i64,
                    "groupInfo": {
                        "groupId": "group-abc-123"
                    }
                }
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert!(msg.is_group);
        assert_eq!(msg.channel_id, "group-abc-123");
        assert_eq!(msg.user_id, "+15559876543");
    }

    #[test]
    fn test_parse_signal_webhook_empty_message() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "envelope": {
                "source": "+15559876543",
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "",
                    "timestamp": 1700000000000_i64,
                    "groupInfo": null
                }
            }
        });

        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[tokio::test]
    async fn test_signal_adapter_start_stop() {
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
