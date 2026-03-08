pub mod discord;
pub mod slack;
pub mod telegram;

pub use discord::DiscordAdapter;
pub use slack::SlackAdapter;
pub use telegram::TelegramAdapter;
