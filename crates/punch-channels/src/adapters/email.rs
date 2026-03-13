//! Email channel adapter via SMTP/IMAP.
//!
//! Sends responses via SMTP using the `lettre` crate.
//! Incoming emails are parsed from IMAP-fetched payloads (polling handled by Arena).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// SMTP/IMAP email configuration.
#[derive(Debug, Clone)]
pub struct EmailConfig {
    /// SMTP server hostname.
    pub smtp_host: String,
    /// SMTP server port (typically 587 for STARTTLS, 465 for TLS).
    pub smtp_port: u16,
    /// SMTP username.
    pub smtp_username: String,
    /// SMTP password.
    pub smtp_password: String,
    /// IMAP server hostname (for receiving emails).
    pub imap_host: String,
    /// IMAP server port (typically 993 for TLS).
    pub imap_port: u16,
    /// The "From" email address.
    pub from_address: String,
    /// Display name for outgoing emails.
    pub from_name: String,
}

/// Email channel adapter.
///
/// Sends responses via SMTP and receives incoming emails via IMAP polling.
pub struct EmailAdapter {
    /// Email configuration.
    config: EmailConfig,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
    /// Last error encountered.
    last_error: RwLock<Option<String>>,
}

impl EmailAdapter {
    /// Create a new email adapter with the given configuration.
    pub fn new(config: EmailConfig) -> Self {
        Self {
            config,
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
            last_error: RwLock::new(None),
        }
    }

    /// Parse an incoming email into an `IncomingMessage`.
    ///
    /// This parses a simplified email representation (as would be extracted
    /// from an IMAP fetch). The caller provides the parsed fields.
    pub fn parse_incoming_email(
        &self,
        from_address: &str,
        from_name: &str,
        subject: &str,
        body: &str,
        message_id: &str,
        date: Option<DateTime<Utc>>,
    ) -> Option<IncomingMessage> {
        if body.is_empty() && subject.is_empty() {
            return None;
        }

        let text = if subject.is_empty() {
            body.to_string()
        } else if body.is_empty() {
            subject.to_string()
        } else {
            format!("[{subject}] {body}")
        };

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: from_address.to_string(),
            user_id: from_address.to_string(),
            display_name: from_name.to_string(),
            text,
            timestamp: date.unwrap_or_else(Utc::now),
            platform: ChannelPlatform::Email,
            platform_message_id: message_id.to_string(),
            is_group: false,
            metadata: HashMap::new(),
        })
    }

    /// Build and send an email via SMTP.
    async fn smtp_send(&self, to_address: &str, text: &str) -> PunchResult<()> {
        let from_mailbox: Mailbox =
            format!("{} <{}>", self.config.from_name, self.config.from_address)
                .parse()
                .map_err(|e| PunchError::Channel {
                    channel: "email".to_string(),
                    message: format!("invalid from address: {e}"),
                })?;

        let to_mailbox: Mailbox = to_address.parse().map_err(|e| PunchError::Channel {
            channel: "email".to_string(),
            message: format!("invalid to address: {e}"),
        })?;

        let email = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject("Re: Punch Agent Response")
            .header(ContentType::TEXT_PLAIN)
            .body(text.to_string())
            .map_err(|e| PunchError::Channel {
                channel: "email".to_string(),
                message: format!("failed to build email: {e}"),
            })?;

        let creds = Credentials::new(
            self.config.smtp_username.clone(),
            self.config.smtp_password.clone(),
        );

        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.config.smtp_host)
                .map_err(|e| PunchError::Channel {
                    channel: "email".to_string(),
                    message: format!("failed to create SMTP transport: {e}"),
                })?
                .port(self.config.smtp_port)
                .credentials(creds)
                .build();

        mailer.send(email).await.map_err(|e| {
            let err_msg = format!("SMTP send failed: {e}");
            warn!("{err_msg}");
            PunchError::Channel {
                channel: "email".to_string(),
                message: err_msg,
            }
        })?;

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for EmailAdapter {
    fn name(&self) -> &str {
        "email"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Email
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(
            smtp_host = %self.config.smtp_host,
            imap_host = %self.config.imap_host,
            "Email adapter started (IMAP polling handled externally)"
        );
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Email adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.smtp_send(channel_id, message).await
    }

    fn status(&self) -> ChannelStatus {
        ChannelStatus {
            connected: self.running.load(Ordering::Relaxed),
            started_at: self.started_at.try_read().ok().and_then(|g| *g),
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            last_error: self.last_error.try_read().ok().and_then(|g| g.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> EmailConfig {
        EmailConfig {
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            smtp_username: "user@example.com".to_string(),
            smtp_password: "password".to_string(),
            imap_host: "imap.example.com".to_string(),
            imap_port: 993,
            from_address: "agent@example.com".to_string(),
            from_name: "Punch Agent".to_string(),
        }
    }

    #[test]
    fn test_email_adapter_creation() {
        let adapter = EmailAdapter::new(make_config());
        assert_eq!(adapter.name(), "email");
        assert_eq!(adapter.platform(), ChannelPlatform::Email);
    }

    #[test]
    fn test_parse_incoming_email_basic() {
        let adapter = EmailAdapter::new(make_config());

        let msg = adapter
            .parse_incoming_email(
                "alice@example.com",
                "Alice",
                "Hello",
                "How are you?",
                "msg-id-123",
                None,
            )
            .unwrap();

        assert_eq!(msg.platform, ChannelPlatform::Email);
        assert_eq!(msg.user_id, "alice@example.com");
        assert_eq!(msg.display_name, "Alice");
        assert_eq!(msg.text, "[Hello] How are you?");
        assert!(!msg.is_group);
    }

    #[test]
    fn test_parse_incoming_email_empty() {
        let adapter = EmailAdapter::new(make_config());

        let msg =
            adapter.parse_incoming_email("alice@example.com", "Alice", "", "", "msg-id-123", None);
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_email_adapter_start_stop() {
        let adapter = EmailAdapter::new(make_config());

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        let status = adapter.status();
        assert!(status.connected);
        assert!(status.started_at.is_some());

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }

    #[test]
    fn test_parse_incoming_email_subject_only() {
        let adapter = EmailAdapter::new(make_config());
        let msg = adapter
            .parse_incoming_email("a@b.com", "A", "Subject only", "", "m1", None)
            .unwrap();
        assert_eq!(msg.text, "Subject only");
    }

    #[test]
    fn test_parse_incoming_email_body_only() {
        let adapter = EmailAdapter::new(make_config());
        let msg = adapter
            .parse_incoming_email("a@b.com", "A", "", "Body only", "m1", None)
            .unwrap();
        assert_eq!(msg.text, "Body only");
    }

    #[test]
    fn test_parse_incoming_email_with_date() {
        let adapter = EmailAdapter::new(make_config());
        let date = chrono::Utc::now();
        let msg = adapter
            .parse_incoming_email("a@b.com", "A", "Subj", "Body", "m1", Some(date))
            .unwrap();
        assert_eq!(msg.timestamp, date);
    }

    #[test]
    fn test_parse_incoming_email_message_counter() {
        let adapter = EmailAdapter::new(make_config());
        assert_eq!(adapter.status().messages_received, 0);
        adapter
            .parse_incoming_email("a@b.com", "A", "S", "B", "m1", None)
            .unwrap();
        assert_eq!(adapter.status().messages_received, 1);
    }

    #[test]
    fn test_parse_incoming_email_platform() {
        let adapter = EmailAdapter::new(make_config());
        let msg = adapter
            .parse_incoming_email("a@b.com", "A", "S", "B", "m1", None)
            .unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Email);
        assert!(!msg.is_group);
        assert_eq!(msg.user_id, "a@b.com");
        assert_eq!(msg.channel_id, "a@b.com");
    }
}
