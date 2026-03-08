pub mod telegram;
pub mod discord;
pub mod slack;

pub use telegram::TelegramAdapter;
pub use discord::DiscordAdapter;
pub use slack::SlackAdapter;
