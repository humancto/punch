use async_trait::async_trait;

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, IncomingMessage};

/// Discord channel adapter (stub).
pub struct DiscordAdapter {
    /// Bot token for the Discord API.
    pub bot_token: String,
}

impl DiscordAdapter {
    pub fn new(bot_token: String) -> Self {
        Self { bot_token }
    }
}

#[async_trait]
impl ChannelAdapter for DiscordAdapter {
    fn name(&self) -> &str {
        "discord"
    }

    async fn connect(&self) -> PunchResult<()> {
        Err(PunchError::Channel {
            channel: "discord".to_string(),
            message: "discord adapter not yet implemented".to_string(),
        })
    }

    async fn send_message(&self, _channel_id: &str, _text: &str) -> PunchResult<()> {
        Err(PunchError::Channel {
            channel: "discord".to_string(),
            message: "discord adapter not yet implemented".to_string(),
        })
    }

    async fn receive_messages(&self) -> PunchResult<Vec<IncomingMessage>> {
        Err(PunchError::Channel {
            channel: "discord".to_string(),
            message: "discord adapter not yet implemented".to_string(),
        })
    }
}
