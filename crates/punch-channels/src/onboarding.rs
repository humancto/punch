//! Channel onboarding guides -- step-by-step setup instructions for each platform.

/// A credential field the user needs to provide.
pub struct CredentialField {
    /// Internal field name (e.g., "bot_token")
    pub name: &'static str,
    /// Environment variable name (e.g., "TELEGRAM_BOT_TOKEN")
    pub env_var: &'static str,
    /// User-facing prompt text
    pub prompt: &'static str,
    /// Whether to hide input (for secrets)
    pub is_secret: bool,
}

/// A single onboarding step.
pub struct OnboardingStep {
    /// Instruction text
    pub instruction: &'static str,
    /// Optional URL to open
    pub url: Option<&'static str>,
}

/// Full onboarding guide for a platform.
pub struct OnboardingGuide {
    pub platform: &'static str,
    pub display_name: &'static str,
    pub steps: Vec<OnboardingStep>,
    pub credentials: Vec<CredentialField>,
    pub webhook_path: &'static str,
}

/// Get the onboarding guide for a platform.
pub fn guide_for(platform: &str) -> Option<OnboardingGuide> {
    match platform.to_lowercase().as_str() {
        "telegram" => Some(OnboardingGuide {
            platform: "telegram",
            display_name: "Telegram",
            steps: vec![
                OnboardingStep {
                    instruction: "Open Telegram and message @BotFather",
                    url: Some("https://t.me/BotFather"),
                },
                OnboardingStep {
                    instruction: "Send /newbot and follow the prompts to create your bot",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Copy the bot token (looks like: 123456789:ABCdefGHIjklMNOpqr)",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Set the webhook URL in BotFather to: https://YOUR_DOMAIN:6660/api/channels/telegram/webhook",
                    url: None,
                },
            ],
            credentials: vec![CredentialField {
                name: "bot_token",
                env_var: "TELEGRAM_BOT_TOKEN",
                prompt: "Paste your Telegram bot token",
                is_secret: true,
            }],
            webhook_path: "/api/channels/telegram/webhook",
        }),
        "slack" => Some(OnboardingGuide {
            platform: "slack",
            display_name: "Slack",
            steps: vec![
                OnboardingStep {
                    instruction: "Go to the Slack API portal and create a new app",
                    url: Some("https://api.slack.com/apps"),
                },
                OnboardingStep {
                    instruction: "Under 'OAuth & Permissions', add bot scopes: chat:write, channels:read, channels:history",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Install the app to your workspace",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Copy the Bot User OAuth Token (starts with xoxb-)",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Under 'Event Subscriptions', set the Request URL to: https://YOUR_DOMAIN:6660/api/channels/slack/webhook",
                    url: None,
                },
            ],
            credentials: vec![CredentialField {
                name: "bot_token",
                env_var: "SLACK_BOT_TOKEN",
                prompt: "Paste your Slack bot token (xoxb-...)",
                is_secret: true,
            }],
            webhook_path: "/api/channels/slack/webhook",
        }),
        "discord" => Some(OnboardingGuide {
            platform: "discord",
            display_name: "Discord",
            steps: vec![
                OnboardingStep {
                    instruction: "Go to the Discord Developer Portal",
                    url: Some("https://discord.com/developers/applications"),
                },
                OnboardingStep {
                    instruction: "Click 'New Application' and give it a name",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Go to the 'Bot' section and click 'Add Bot'",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Enable 'Message Content Intent' under Privileged Gateway Intents",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Copy the bot token",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Use the OAuth2 URL Generator to invite the bot to your server with 'Send Messages' permission",
                    url: None,
                },
            ],
            credentials: vec![CredentialField {
                name: "bot_token",
                env_var: "DISCORD_BOT_TOKEN",
                prompt: "Paste your Discord bot token",
                is_secret: true,
            }],
            webhook_path: "/api/channels/discord/webhook",
        }),
        "whatsapp" => Some(OnboardingGuide {
            platform: "whatsapp",
            display_name: "WhatsApp Business",
            steps: vec![
                OnboardingStep {
                    instruction: "Go to Meta for Developers",
                    url: Some("https://developers.facebook.com"),
                },
                OnboardingStep {
                    instruction: "Create a new app and select 'Business' type",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Add the WhatsApp product to your app",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Get your Phone Number ID and Access Token from the WhatsApp settings",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Configure the webhook URL to: https://YOUR_DOMAIN:6660/api/channels/whatsapp/webhook",
                    url: None,
                },
            ],
            credentials: vec![
                CredentialField {
                    name: "access_token",
                    env_var: "WHATSAPP_ACCESS_TOKEN",
                    prompt: "Paste your WhatsApp access token",
                    is_secret: true,
                },
                CredentialField {
                    name: "phone_number_id",
                    env_var: "WHATSAPP_PHONE_NUMBER_ID",
                    prompt: "Paste your Phone Number ID",
                    is_secret: false,
                },
            ],
            webhook_path: "/api/channels/whatsapp/webhook",
        }),
        "github" => Some(OnboardingGuide {
            platform: "github",
            display_name: "GitHub",
            steps: vec![
                OnboardingStep {
                    instruction: "Go to GitHub Settings > Developer settings > Personal access tokens",
                    url: Some("https://github.com/settings/tokens"),
                },
                OnboardingStep {
                    instruction: "Generate a new token (classic) with 'repo' scope",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Copy the token (starts with ghp_)",
                    url: None,
                },
                OnboardingStep {
                    instruction: "Set up a webhook on your repo pointing to: https://YOUR_DOMAIN:6660/api/channels/github/webhook",
                    url: None,
                },
            ],
            credentials: vec![CredentialField {
                name: "token",
                env_var: "GITHUB_TOKEN",
                prompt: "Paste your GitHub personal access token",
                is_secret: true,
            }],
            webhook_path: "/api/channels/github/webhook",
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guide_for_telegram() {
        let guide = guide_for("telegram").unwrap();
        assert_eq!(guide.platform, "telegram");
        assert_eq!(guide.display_name, "Telegram");
        assert!(!guide.steps.is_empty());
        assert_eq!(guide.credentials.len(), 1);
        assert_eq!(guide.credentials[0].name, "bot_token");
        assert!(guide.credentials[0].is_secret);
    }

    #[test]
    fn test_guide_for_slack() {
        let guide = guide_for("slack").unwrap();
        assert_eq!(guide.platform, "slack");
        assert_eq!(guide.credentials[0].env_var, "SLACK_BOT_TOKEN");
    }

    #[test]
    fn test_guide_for_discord() {
        let guide = guide_for("discord").unwrap();
        assert_eq!(guide.platform, "discord");
        assert_eq!(guide.display_name, "Discord");
    }

    #[test]
    fn test_guide_for_whatsapp() {
        let guide = guide_for("whatsapp").unwrap();
        assert_eq!(guide.platform, "whatsapp");
        assert_eq!(guide.credentials.len(), 2);
        assert!(!guide.credentials[1].is_secret);
    }

    #[test]
    fn test_guide_for_github() {
        let guide = guide_for("github").unwrap();
        assert_eq!(guide.platform, "github");
        assert_eq!(guide.credentials[0].name, "token");
    }

    #[test]
    fn test_guide_for_unknown_returns_none() {
        assert!(guide_for("unknown_platform").is_none());
    }

    #[test]
    fn test_guide_for_case_insensitive() {
        assert!(guide_for("Telegram").is_some());
        assert!(guide_for("SLACK").is_some());
        assert!(guide_for("Discord").is_some());
    }

    #[test]
    fn test_all_guides_have_webhook_path() {
        for platform in &["telegram", "slack", "discord", "whatsapp", "github"] {
            let guide = guide_for(platform).unwrap();
            assert!(guide.webhook_path.starts_with("/api/channels/"));
        }
    }

    #[test]
    fn test_all_guides_have_steps_with_first_url() {
        for platform in &["telegram", "slack", "discord", "whatsapp", "github"] {
            let guide = guide_for(platform).unwrap();
            assert!(
                guide.steps[0].url.is_some(),
                "{} first step should have a URL",
                platform
            );
        }
    }
}
