use async_trait::async_trait;

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, IncomingMessage};

/// Telegram channel adapter (stub).
pub struct TelegramAdapter {
    /// Bot token for the Telegram Bot API.
    pub bot_token: String,
}

impl TelegramAdapter {
    pub fn new(bot_token: String) -> Self {
        Self { bot_token }
    }
}

#[async_trait]
impl ChannelAdapter for TelegramAdapter {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn connect(&self) -> PunchResult<()> {
        Err(PunchError::Channel {
            channel: "telegram".to_string(),
            message: "telegram adapter not yet implemented".to_string(),
        })
    }

    async fn send_message(&self, _channel_id: &str, _text: &str) -> PunchResult<()> {
        Err(PunchError::Channel {
            channel: "telegram".to_string(),
            message: "telegram adapter not yet implemented".to_string(),
        })
    }

    async fn receive_messages(&self) -> PunchResult<Vec<IncomingMessage>> {
        Err(PunchError::Channel {
            channel: "telegram".to_string(),
            message: "telegram adapter not yet implemented".to_string(),
        })
    }
}
