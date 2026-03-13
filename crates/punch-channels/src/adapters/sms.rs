//! SMS adapter via Twilio.
//!
//! Sends SMS/MMS via the Twilio REST API and parses incoming Twilio
//! webhook payloads for received messages.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const TWILIO_API_BASE: &str = "https://api.twilio.com/2010-04-01";

/// Twilio SMS/MMS adapter.
///
/// Sends SMS and MMS messages via the Twilio REST API.
/// Receives messages by parsing Twilio webhook (TwiML) payloads.
pub struct SmsAdapter {
    /// Twilio Account SID.
    account_sid: String,
    /// Twilio Auth Token.
    auth_token: String,
    /// Twilio phone number (E.164 format, e.g. "+15551234567").
    from_number: String,
    /// HTTP client.
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl SmsAdapter {
    /// Create a new SMS adapter with Twilio credentials.
    pub fn new(account_sid: String, auth_token: String, from_number: String) -> Self {
        Self {
            account_sid,
            auth_token,
            from_number,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Send an SMS message via Twilio REST API.
    pub async fn send_sms(&self, to: &str, body: &str) -> PunchResult<()> {
        self.send_message_internal(to, body, None).await
    }

    /// Send an MMS message with media URLs via Twilio REST API.
    pub async fn send_mms(&self, to: &str, body: &str, media_urls: &[String]) -> PunchResult<()> {
        self.send_message_internal(to, body, Some(media_urls)).await
    }

    async fn send_message_internal(
        &self,
        to: &str,
        body: &str,
        media_urls: Option<&[String]>,
    ) -> PunchResult<()> {
        let url = format!(
            "{}/Accounts/{}/Messages.json",
            TWILIO_API_BASE, self.account_sid
        );

        let mut params = vec![
            ("To", to.to_string()),
            ("From", self.from_number.clone()),
            ("Body", body.to_string()),
        ];

        if let Some(urls) = media_urls {
            for media_url in urls {
                params.push(("MediaUrl", media_url.clone()));
            }
        }

        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.account_sid, Some(&self.auth_token))
            .form(&params)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "sms".to_string(),
                message: format!("failed to send SMS: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Twilio send failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Parse a Twilio webhook payload (form data) into an `IncomingMessage`.
    ///
    /// Twilio sends POST requests with form-encoded parameters including:
    /// - `MessageSid`: Unique message identifier
    /// - `From`: Sender phone number
    /// - `To`: Recipient phone number
    /// - `Body`: Message text
    /// - `NumMedia`: Number of media attachments
    /// - `MediaUrl0`, `MediaUrl1`, ...: Media URLs
    pub fn parse_webhook_payload(
        &self,
        params: &HashMap<String, String>,
    ) -> Option<IncomingMessage> {
        let from = params.get("From")?;
        let body = params.get("Body")?;
        if body.is_empty() {
            return None;
        }

        let message_sid = params.get("MessageSid").cloned().unwrap_or_default();
        let to = params.get("To").cloned().unwrap_or_default();

        let mut metadata = HashMap::new();
        metadata.insert(
            "to".to_string(),
            serde_json::Value::String(to),
        );

        // Collect media URLs if present
        let num_media: usize = params
            .get("NumMedia")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        if num_media > 0 {
            let mut media_urls = Vec::new();
            for i in 0..num_media {
                if let Some(url) = params.get(&format!("MediaUrl{i}")) {
                    media_urls.push(serde_json::Value::String(url.clone()));
                }
            }
            if !media_urls.is_empty() {
                metadata.insert(
                    "media_urls".to_string(),
                    serde_json::Value::Array(media_urls),
                );
            }
        }

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: from.clone(),
            user_id: from.clone(),
            display_name: from.clone(),
            text: body.clone(),
            timestamp: Utc::now(),
            platform: ChannelPlatform::Sms,
            platform_message_id: message_sid,
            is_group: false,
            metadata,
        })
    }
}

#[async_trait]
impl ChannelAdapter for SmsAdapter {
    fn name(&self) -> &str {
        "sms"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Sms
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(from = %self.from_number, "SMS adapter started (Twilio)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("SMS adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.send_sms(channel_id, message).await
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

    fn make_adapter() -> SmsAdapter {
        SmsAdapter::new(
            "AC1234567890".to_string(),
            "auth-token-secret".to_string(),
            "+15551234567".to_string(),
        )
    }

    #[test]
    fn test_sms_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "sms");
        assert_eq!(adapter.platform(), ChannelPlatform::Sms);
    }

    #[test]
    fn test_parse_webhook_sms() {
        let adapter = make_adapter();
        let mut params = HashMap::new();
        params.insert("MessageSid".to_string(), "SM123".to_string());
        params.insert("From".to_string(), "+15559876543".to_string());
        params.insert("To".to_string(), "+15551234567".to_string());
        params.insert("Body".to_string(), "Hello from SMS".to_string());
        params.insert("NumMedia".to_string(), "0".to_string());

        let msg = adapter.parse_webhook_payload(&params).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Sms);
        assert_eq!(msg.user_id, "+15559876543");
        assert_eq!(msg.text, "Hello from SMS");
        assert_eq!(msg.platform_message_id, "SM123");
    }

    #[test]
    fn test_parse_webhook_mms_with_media() {
        let adapter = make_adapter();
        let mut params = HashMap::new();
        params.insert("MessageSid".to_string(), "MM456".to_string());
        params.insert("From".to_string(), "+15559876543".to_string());
        params.insert("To".to_string(), "+15551234567".to_string());
        params.insert("Body".to_string(), "Check this out".to_string());
        params.insert("NumMedia".to_string(), "2".to_string());
        params.insert(
            "MediaUrl0".to_string(),
            "https://api.twilio.com/media/img1.jpg".to_string(),
        );
        params.insert(
            "MediaUrl1".to_string(),
            "https://api.twilio.com/media/img2.jpg".to_string(),
        );

        let msg = adapter.parse_webhook_payload(&params).unwrap();
        assert_eq!(msg.text, "Check this out");
        let media = msg.metadata.get("media_urls").unwrap().as_array().unwrap();
        assert_eq!(media.len(), 2);
    }

    #[test]
    fn test_parse_webhook_empty_body() {
        let adapter = make_adapter();
        let mut params = HashMap::new();
        params.insert("From".to_string(), "+15559876543".to_string());
        params.insert("Body".to_string(), String::new());
        assert!(adapter.parse_webhook_payload(&params).is_none());
    }

    #[tokio::test]
    async fn test_sms_start_stop() {
        let adapter = make_adapter();
        assert!(!adapter.status().connected);
        adapter.start().await.unwrap();
        assert!(adapter.status().connected);
        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
