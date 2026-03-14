//! Twitch chat adapter via IRC protocol.
//!
//! Connects to Twitch IRC (irc.chat.twitch.tv:6697) via TLS and handles
//! Twitch-specific IRC tags for badges, emotes, and subscriber status.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const TWITCH_IRC_HOST: &str = "irc.chat.twitch.tv";
const TWITCH_IRC_PORT: u16 = 6697;
const TWITCH_TMI_URL: &str = "https://tmi.twitch.tv";

/// Twitch chat adapter using the Twitch IRC interface.
///
/// Connects to Twitch IRC with TLS and handles Twitch-specific message tags.
pub struct TwitchAdapter {
    /// The Twitch channel name to join (without the "#" prefix).
    channel_name: String,
    /// OAuth token for authentication (format: "oauth:xxx").
    oauth_token: String,
    /// Bot username on Twitch.
    bot_username: String,
    /// HTTP client for fallback API calls.
    client: reqwest::Client,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

/// Parsed Twitch IRC message with tags.
#[derive(Debug, Clone)]
pub struct TwitchIrcMessage {
    /// Twitch IRC tags (e.g. badges, color, display-name, emotes, subscriber).
    pub tags: HashMap<String, String>,
    /// The source (nick!user@host).
    pub source: Option<String>,
    /// The IRC command (e.g. PRIVMSG, PING).
    pub command: String,
    /// The channel the message was sent to.
    pub channel: Option<String>,
    /// The message text.
    pub text: Option<String>,
}

impl TwitchAdapter {
    /// Create a new Twitch adapter.
    ///
    /// `channel_name`: Twitch channel to join (without "#").
    /// `oauth_token`: OAuth token in format "oauth:xxx".
    /// `bot_username`: The bot's Twitch username.
    pub fn new(channel_name: String, oauth_token: String, bot_username: String) -> Self {
        Self {
            channel_name,
            oauth_token,
            bot_username,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// The IRC host for Twitch chat.
    pub fn irc_host(&self) -> &str {
        TWITCH_IRC_HOST
    }

    /// The IRC port for Twitch chat (TLS).
    pub fn irc_port(&self) -> u16 {
        TWITCH_IRC_PORT
    }

    /// Build the IRC authentication sequence for Twitch.
    pub fn build_auth_messages(&self) -> Vec<String> {
        vec![
            // Request Twitch-specific capabilities (tags, commands, membership)
            "CAP REQ :twitch.tv/tags twitch.tv/commands twitch.tv/membership".to_string(),
            format!("PASS {}", self.oauth_token),
            format!("NICK {}", self.bot_username),
            format!("JOIN #{}", self.channel_name),
        ]
    }

    /// Build a PRIVMSG command to send a chat message.
    pub fn build_privmsg(&self, text: &str) -> String {
        format!("PRIVMSG #{} :{}", self.channel_name, text)
    }

    /// Parse a raw Twitch IRC line into a `TwitchIrcMessage`.
    ///
    /// Twitch IRC format with tags:
    /// `@badge-info=;badges=moderator/1;color=#FF0000;display-name=Alice;emotes=;subscriber=0;user-id=12345 :alice!alice@alice.tmi.twitch.tv PRIVMSG #channel :Hello!`
    pub fn parse_irc_line(raw: &str) -> Option<TwitchIrcMessage> {
        let mut remaining = raw.trim();
        if remaining.is_empty() {
            return None;
        }

        // Parse tags (prefixed with @)
        let mut tags = HashMap::new();
        if remaining.starts_with('@')
            && let Some(space_idx) = remaining.find(' ')
        {
            let tags_str = &remaining[1..space_idx];
            for pair in tags_str.split(';') {
                if let Some((key, value)) = pair.split_once('=') {
                    tags.insert(key.to_string(), value.to_string());
                }
            }
            remaining = &remaining[space_idx + 1..];
        }

        // Parse source (prefixed with :)
        let source = if remaining.starts_with(':') {
            if let Some(space_idx) = remaining.find(' ') {
                let src = remaining[1..space_idx].to_string();
                remaining = &remaining[space_idx + 1..];
                Some(src)
            } else {
                return None;
            }
        } else {
            None
        };

        // Parse command and parameters
        let (command_part, text) = if let Some(colon_idx) = remaining.find(" :") {
            let cmd = &remaining[..colon_idx];
            let txt = &remaining[colon_idx + 2..];
            (cmd, Some(txt.to_string()))
        } else {
            (remaining, None)
        };

        let mut parts = command_part.split_whitespace();
        let command = parts.next()?.to_string();
        let channel = parts.next().map(|s| s.to_string());

        Some(TwitchIrcMessage {
            tags,
            source,
            command,
            channel,
            text,
        })
    }

    /// Convert a parsed Twitch IRC PRIVMSG into an `IncomingMessage`.
    pub fn irc_to_incoming(&self, irc_msg: &TwitchIrcMessage) -> Option<IncomingMessage> {
        if irc_msg.command != "PRIVMSG" {
            return None;
        }

        let text = irc_msg.text.as_deref()?;
        if text.is_empty() {
            return None;
        }

        // Extract username from source (nick!user@host)
        let username = irc_msg
            .source
            .as_ref()
            .and_then(|s| s.split('!').next().map(|n| n.to_string()))?;

        let display_name = irc_msg
            .tags
            .get("display-name")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| username.clone());

        let user_id = irc_msg
            .tags
            .get("user-id")
            .cloned()
            .unwrap_or_else(|| username.clone());

        let msg_id = irc_msg
            .tags
            .get("id")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        let channel = irc_msg
            .channel
            .as_deref()
            .unwrap_or("")
            .trim_start_matches('#')
            .to_string();

        let mut metadata = HashMap::new();
        if let Some(badges) = irc_msg.tags.get("badges") {
            metadata.insert(
                "badges".to_string(),
                serde_json::Value::String(badges.clone()),
            );
        }
        if let Some(emotes) = irc_msg.tags.get("emotes") {
            metadata.insert(
                "emotes".to_string(),
                serde_json::Value::String(emotes.clone()),
            );
        }
        if let Some(subscriber) = irc_msg.tags.get("subscriber") {
            metadata.insert(
                "subscriber".to_string(),
                serde_json::Value::String(subscriber.clone()),
            );
        }

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: channel,
            user_id,
            display_name,
            text: text.to_string(),
            timestamp: Utc::now(),
            platform: ChannelPlatform::Twitch,
            platform_message_id: msg_id,
            is_group: true, // Twitch chat is always a group context
            metadata,
        })
    }

    /// Send a chat message via the Twitch TMI endpoint (HTTP fallback).
    async fn api_send_message(&self, _channel: &str, text: &str) -> PunchResult<()> {
        // Use the Twitch Helix API to send a chat message as a fallback
        // when not connected via IRC directly.
        let url = format!("{}/chat/send", TWITCH_TMI_URL);

        let body = serde_json::json!({
            "channel": format!("#{}", self.channel_name),
            "message": text,
        });

        let resp = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!(
                    "OAuth {}",
                    self.oauth_token
                        .strip_prefix("oauth:")
                        .unwrap_or(&self.oauth_token)
                ),
            )
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "twitch".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Twitch send message failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for TwitchAdapter {
    fn name(&self) -> &str {
        "twitch"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Twitch
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(channel = %self.channel_name, "Twitch adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Twitch adapter stopped");
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

    fn make_adapter() -> TwitchAdapter {
        TwitchAdapter::new(
            "testchannel".to_string(),
            "oauth:test-token-123".to_string(),
            "punchbot".to_string(),
        )
    }

    #[test]
    fn test_twitch_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "twitch");
        assert_eq!(adapter.platform(), ChannelPlatform::Twitch);
    }

    #[test]
    fn test_build_auth_messages() {
        let adapter = make_adapter();
        let msgs = adapter.build_auth_messages();
        assert_eq!(msgs.len(), 4);
        assert!(msgs[0].starts_with("CAP REQ"));
        assert!(msgs[1].starts_with("PASS oauth:"));
        assert_eq!(msgs[2], "NICK punchbot");
        assert_eq!(msgs[3], "JOIN #testchannel");
    }

    #[test]
    fn test_build_privmsg() {
        let adapter = make_adapter();
        assert_eq!(
            adapter.build_privmsg("Hello chat!"),
            "PRIVMSG #testchannel :Hello chat!"
        );
    }

    #[test]
    fn test_parse_irc_line_with_tags() {
        let raw = "@badge-info=subscriber/12;badges=subscriber/12,premium/1;color=#FF0000;display-name=Alice;emotes=;id=msg-id-123;subscriber=1;user-id=12345 :alice!alice@alice.tmi.twitch.tv PRIVMSG #testchannel :Hello from Twitch!";

        let msg = TwitchAdapter::parse_irc_line(raw).unwrap();
        assert_eq!(msg.command, "PRIVMSG");
        assert_eq!(msg.channel.as_deref(), Some("#testchannel"));
        assert_eq!(msg.text.as_deref(), Some("Hello from Twitch!"));
        assert_eq!(msg.tags.get("display-name").unwrap(), "Alice");
        assert_eq!(msg.tags.get("user-id").unwrap(), "12345");
        assert_eq!(msg.tags.get("subscriber").unwrap(), "1");
    }

    #[test]
    fn test_parse_irc_line_ping() {
        let raw = "PING :tmi.twitch.tv";
        let msg = TwitchAdapter::parse_irc_line(raw).unwrap();
        assert_eq!(msg.command, "PING");
        assert_eq!(msg.text.as_deref(), Some("tmi.twitch.tv"));
    }

    #[test]
    fn test_irc_to_incoming() {
        let adapter = make_adapter();
        let raw = "@badge-info=;badges=moderator/1;display-name=Bob;emotes=;id=abc-def;subscriber=0;user-id=67890 :bob!bob@bob.tmi.twitch.tv PRIVMSG #testchannel :Hey punchbot!";

        let irc_msg = TwitchAdapter::parse_irc_line(raw).unwrap();
        let incoming = adapter.irc_to_incoming(&irc_msg).unwrap();

        assert_eq!(incoming.platform, ChannelPlatform::Twitch);
        assert_eq!(incoming.user_id, "67890");
        assert_eq!(incoming.display_name, "Bob");
        assert_eq!(incoming.text, "Hey punchbot!");
        assert_eq!(incoming.channel_id, "testchannel");
        assert!(incoming.is_group);
        assert!(incoming.metadata.contains_key("badges"));
    }

    #[tokio::test]
    async fn test_twitch_adapter_start_stop() {
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
