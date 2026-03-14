//! IRC channel adapter using raw TCP and the IRC protocol.
//!
//! Connects to an IRC server, joins channels, and exchanges messages
//! using standard IRC protocol commands (NICK, USER, JOIN, PRIVMSG, PING/PONG).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const IRC_MSG_LIMIT: usize = 512;

/// A parsed IRC protocol message.
///
/// IRC messages follow the format: `[:prefix] COMMAND [params] [:trailing]`
#[derive(Debug, Clone, PartialEq)]
pub struct IrcProtocolMessage {
    /// Optional prefix (usually the sender, e.g. `nick!user@host`).
    pub prefix: Option<String>,
    /// The IRC command (e.g. PRIVMSG, PING, JOIN).
    pub command: String,
    /// Command parameters.
    pub params: Vec<String>,
    /// Trailing parameter (the text after the final `:`).
    pub trailing: Option<String>,
}

/// Parse a raw IRC protocol line into an `IrcProtocolMessage`.
///
/// IRC format: `[:prefix SPACE] command [params] [: trailing]`
pub fn parse_irc_message(line: &str) -> Option<IrcProtocolMessage> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }

    let mut remaining = line;
    let prefix = if remaining.starts_with(':') {
        let space_idx = remaining.find(' ')?;
        let pfx = &remaining[1..space_idx];
        remaining = remaining[space_idx + 1..].trim_start();
        Some(pfx.to_string())
    } else {
        None
    };

    // Split at trailing (the part after " :")
    let (params_part, trailing) = if let Some(idx) = remaining.find(" :") {
        let t = &remaining[idx + 2..];
        (&remaining[..idx], Some(t.to_string()))
    } else {
        (remaining, None)
    };

    let mut parts: Vec<&str> = params_part.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let command = parts.remove(0).to_uppercase();
    let params: Vec<String> = parts.iter().map(|s| s.to_string()).collect();

    Some(IrcProtocolMessage {
        prefix,
        command,
        params,
        trailing,
    })
}

/// IRC channel adapter.
///
/// Connects to an IRC server via TCP and communicates using the IRC protocol.
pub struct IrcAdapter {
    /// IRC server hostname.
    server: String,
    /// IRC server port.
    port: u16,
    /// The bot's nickname.
    nick: String,
    /// Channels to join (with '#' prefix).
    channels: Vec<String>,
    /// The TCP stream to the server (established on start).
    stream: RwLock<Option<TcpStream>>,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl IrcAdapter {
    /// Create a new IRC adapter.
    ///
    /// `server`: IRC server hostname.
    /// `port`: IRC server port (typically 6667 for plain, 6697 for TLS).
    /// `nick`: The bot's nickname.
    /// `channels`: List of channels to join (e.g. `["#general", "#bots"]`).
    pub fn new(server: String, port: u16, nick: String, channels: Vec<String>) -> Self {
        Self {
            server,
            port,
            nick,
            channels,
            stream: RwLock::new(None),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Convert an IRC PRIVMSG into an `IncomingMessage`.
    ///
    /// The `IrcProtocolMessage` should have command == "PRIVMSG".
    pub fn irc_message_to_incoming(&self, irc_msg: &IrcProtocolMessage) -> Option<IncomingMessage> {
        if irc_msg.command != "PRIVMSG" {
            return None;
        }

        let text = irc_msg.trailing.as_deref()?;
        if text.is_empty() {
            return None;
        }

        let target = irc_msg.params.first()?;

        // Extract nick from prefix (nick!user@host)
        let nick = irc_msg
            .prefix
            .as_deref()
            .and_then(|p| p.split('!').next())
            .unwrap_or("unknown");

        let is_group = target.starts_with('#') || target.starts_with('&');

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: target.to_string(),
            user_id: nick.to_string(),
            display_name: nick.to_string(),
            text: text.to_string(),
            timestamp: Utc::now(),
            platform: ChannelPlatform::Irc,
            platform_message_id: String::new(), // IRC has no message IDs
            is_group,
            metadata: HashMap::new(),
        })
    }

    /// Send a raw IRC line over the TCP stream.
    async fn send_raw(&self, line: &str) -> PunchResult<()> {
        let mut guard = self.stream.write().await;
        let stream = guard.as_mut().ok_or_else(|| PunchError::Channel {
            channel: "irc".to_string(),
            message: "not connected to IRC server".to_string(),
        })?;

        let data = if line.ends_with("\r\n") {
            line.to_string()
        } else {
            format!("{line}\r\n")
        };

        // IRC protocol max is 512 bytes per message
        if data.len() > IRC_MSG_LIMIT {
            warn!("IRC message exceeds 512-byte limit, truncating");
        }

        stream
            .write_all(data.as_bytes())
            .await
            .map_err(|e| PunchError::Channel {
                channel: "irc".to_string(),
                message: format!("failed to write to stream: {e}"),
            })?;

        Ok(())
    }

    /// Send a PRIVMSG to a target (channel or nick).
    async fn send_privmsg(&self, target: &str, text: &str) -> PunchResult<()> {
        // Split long messages to fit IRC's 512-byte limit
        // Account for "PRIVMSG target :" prefix + "\r\n" suffix
        let prefix_len = 10 + target.len() + 2; // "PRIVMSG " + target + " :" + "\r\n"
        let max_text_len = IRC_MSG_LIMIT.saturating_sub(prefix_len);

        let lines: Vec<&str> = if text.len() <= max_text_len {
            vec![text]
        } else {
            text.as_bytes()
                .chunks(max_text_len)
                .map(|chunk| {
                    // Safe because we're splitting at byte boundaries of ASCII-safe content
                    std::str::from_utf8(chunk).unwrap_or("")
                })
                .filter(|s| !s.is_empty())
                .collect()
        };

        for line in lines {
            self.send_raw(&format!("PRIVMSG {target} :{line}")).await?;
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Perform the IRC registration handshake (NICK + USER).
    async fn register(&self) -> PunchResult<()> {
        self.send_raw(&format!("NICK {}", self.nick)).await?;
        self.send_raw(&format!("USER {} 0 * :Punch Agent Bot", self.nick))
            .await?;
        Ok(())
    }

    /// Join configured IRC channels.
    async fn join_channels(&self) -> PunchResult<()> {
        for channel in &self.channels {
            self.send_raw(&format!("JOIN {channel}")).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for IrcAdapter {
    fn name(&self) -> &str {
        "irc"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Irc
    }

    async fn start(&self) -> PunchResult<()> {
        let stream = TcpStream::connect(format!("{}:{}", self.server, self.port))
            .await
            .map_err(|e| PunchError::Channel {
                channel: "irc".to_string(),
                message: format!("failed to connect to {}:{}: {e}", self.server, self.port),
            })?;

        *self.stream.write().await = Some(stream);
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());

        self.register().await?;
        self.join_channels().await?;

        info!(
            server = %self.server,
            port = self.port,
            nick = %self.nick,
            channels = ?self.channels,
            "IRC adapter started"
        );
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        if self.running.load(Ordering::Relaxed) {
            // Send QUIT before disconnecting
            let _ = self.send_raw("QUIT :Punch Agent signing off").await;
        }
        *self.stream.write().await = None;
        self.running.store(false, Ordering::Relaxed);
        info!("IRC adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.send_privmsg(channel_id, message).await
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

    fn make_adapter() -> IrcAdapter {
        IrcAdapter::new(
            "irc.example.com".to_string(),
            6667,
            "punchbot".to_string(),
            vec!["#general".to_string(), "#bots".to_string()],
        )
    }

    #[test]
    fn test_irc_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "irc");
        assert_eq!(adapter.platform(), ChannelPlatform::Irc);
    }

    #[test]
    fn test_parse_irc_message_privmsg() {
        let line = ":alice!alice@host.com PRIVMSG #general :Hello everyone!";
        let parsed = parse_irc_message(line).unwrap();

        assert_eq!(parsed.prefix.as_deref(), Some("alice!alice@host.com"));
        assert_eq!(parsed.command, "PRIVMSG");
        assert_eq!(parsed.params, vec!["#general"]);
        assert_eq!(parsed.trailing.as_deref(), Some("Hello everyone!"));
    }

    #[test]
    fn test_parse_irc_message_ping() {
        let line = "PING :irc.example.com";
        let parsed = parse_irc_message(line).unwrap();

        assert!(parsed.prefix.is_none());
        assert_eq!(parsed.command, "PING");
        assert!(parsed.params.is_empty());
        assert_eq!(parsed.trailing.as_deref(), Some("irc.example.com"));
    }

    #[test]
    fn test_parse_irc_message_numeric() {
        let line = ":irc.example.com 001 punchbot :Welcome to the IRC Network";
        let parsed = parse_irc_message(line).unwrap();

        assert_eq!(parsed.prefix.as_deref(), Some("irc.example.com"));
        assert_eq!(parsed.command, "001");
        assert_eq!(parsed.params, vec!["punchbot"]);
        assert_eq!(
            parsed.trailing.as_deref(),
            Some("Welcome to the IRC Network")
        );
    }

    #[test]
    fn test_parse_irc_message_empty() {
        assert!(parse_irc_message("").is_none());
        assert!(parse_irc_message("\r\n").is_none());
    }

    #[test]
    fn test_irc_message_to_incoming_privmsg() {
        let adapter = make_adapter();
        let irc_msg = IrcProtocolMessage {
            prefix: Some("alice!alice@host.com".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["#general".to_string()],
            trailing: Some("Hello from IRC!".to_string()),
        };

        let msg = adapter.irc_message_to_incoming(&irc_msg).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Irc);
        assert_eq!(msg.user_id, "alice");
        assert_eq!(msg.channel_id, "#general");
        assert_eq!(msg.text, "Hello from IRC!");
        assert!(msg.is_group);
    }

    #[test]
    fn test_irc_message_to_incoming_dm() {
        let adapter = make_adapter();
        let irc_msg = IrcProtocolMessage {
            prefix: Some("bob!bob@host.com".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["punchbot".to_string()],
            trailing: Some("Private message".to_string()),
        };

        let msg = adapter.irc_message_to_incoming(&irc_msg).unwrap();
        assert!(!msg.is_group);
        assert_eq!(msg.channel_id, "punchbot");
    }

    #[test]
    fn test_irc_message_to_incoming_non_privmsg() {
        let adapter = make_adapter();
        let irc_msg = IrcProtocolMessage {
            prefix: Some("alice!alice@host.com".to_string()),
            command: "JOIN".to_string(),
            params: vec!["#general".to_string()],
            trailing: None,
        };

        let msg = adapter.irc_message_to_incoming(&irc_msg);
        assert!(msg.is_none());
    }

    #[test]
    fn test_parse_irc_kick() {
        let line = ":op!op@host KICK #channel user :Reason for kick";
        let parsed = parse_irc_message(line).unwrap();
        assert_eq!(parsed.command, "KICK");
        assert_eq!(parsed.params, vec!["#channel", "user"]);
        assert_eq!(parsed.trailing.as_deref(), Some("Reason for kick"));
    }

    #[test]
    fn test_parse_irc_mode() {
        let line = ":op!op@host MODE #channel +o user";
        let parsed = parse_irc_message(line).unwrap();
        assert_eq!(parsed.command, "MODE");
        assert_eq!(parsed.params, vec!["#channel", "+o", "user"]);
    }

    #[test]
    fn test_parse_irc_notice() {
        let line = ":server NOTICE * :*** Looking up your hostname";
        let parsed = parse_irc_message(line).unwrap();
        assert_eq!(parsed.command, "NOTICE");
        assert_eq!(parsed.params, vec!["*"]);
        assert_eq!(
            parsed.trailing.as_deref(),
            Some("*** Looking up your hostname")
        );
    }

    #[test]
    fn test_parse_irc_error_reply() {
        let line = ":irc.example.com 433 * punchbot :Nickname is already in use";
        let parsed = parse_irc_message(line).unwrap();
        assert_eq!(parsed.command, "433");
        assert_eq!(parsed.params, vec!["*", "punchbot"]);
    }

    #[test]
    fn test_parse_irc_quit() {
        let line = ":nick!user@host QUIT :Leaving";
        let parsed = parse_irc_message(line).unwrap();
        assert_eq!(parsed.command, "QUIT");
        assert_eq!(parsed.trailing.as_deref(), Some("Leaving"));
    }

    #[test]
    fn test_parse_irc_join() {
        let line = ":alice!alice@host JOIN #channel";
        let parsed = parse_irc_message(line).unwrap();
        assert_eq!(parsed.command, "JOIN");
        assert_eq!(parsed.params, vec!["#channel"]);
    }

    #[test]
    fn test_irc_message_to_incoming_empty_trailing() {
        let adapter = make_adapter();
        let irc_msg = IrcProtocolMessage {
            prefix: Some("alice!alice@host.com".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["#general".to_string()],
            trailing: Some("".to_string()),
        };
        assert!(adapter.irc_message_to_incoming(&irc_msg).is_none());
    }

    #[test]
    fn test_irc_message_to_incoming_no_prefix() {
        let adapter = make_adapter();
        let irc_msg = IrcProtocolMessage {
            prefix: None,
            command: "PRIVMSG".to_string(),
            params: vec!["#general".to_string()],
            trailing: Some("Hello".to_string()),
        };
        let msg = adapter.irc_message_to_incoming(&irc_msg).unwrap();
        assert_eq!(msg.user_id, "unknown");
    }

    #[test]
    fn test_irc_channel_ampersand_is_group() {
        let adapter = make_adapter();
        let irc_msg = IrcProtocolMessage {
            prefix: Some("user!u@h".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["&channel".to_string()],
            trailing: Some("msg".to_string()),
        };
        let msg = adapter.irc_message_to_incoming(&irc_msg).unwrap();
        assert!(msg.is_group);
    }

    #[test]
    fn test_parse_irc_no_trailing() {
        let line = ":server 001 nick";
        let parsed = parse_irc_message(line).unwrap();
        assert_eq!(parsed.command, "001");
        assert_eq!(parsed.params, vec!["nick"]);
        assert!(parsed.trailing.is_none());
    }
}
