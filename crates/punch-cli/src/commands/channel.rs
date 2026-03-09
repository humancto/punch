//! `punch channel` — Manage channel adapters.

use crate::cli::ChannelCommands;

pub async fn run(command: ChannelCommands, config_path: Option<String>) -> i32 {
    match command {
        ChannelCommands::List => run_list(config_path).await,
        ChannelCommands::Test { platform } => run_test(&platform, config_path).await,
    }
}

async fn run_list(config_path: Option<String>) -> i32 {
    let config = match super::load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    if config.channels.is_empty() {
        println!("  No channels configured.");
        println!();
        println!("  Add channels to ~/.punch/config.toml:");
        println!();
        println!("  [channels.telegram]");
        println!("  channel_type = \"telegram\"");
        println!("  token_env = \"TELEGRAM_BOT_TOKEN\"");
        println!();
        println!("  [channels.discord]");
        println!("  channel_type = \"discord\"");
        println!("  token_env = \"DISCORD_BOT_TOKEN\"");
        println!();
        println!("  [channels.slack]");
        println!("  channel_type = \"slack\"");
        println!("  token_env = \"SLACK_BOT_TOKEN\"");
        return 0;
    }

    println!("  Configured channels:");
    println!();

    for (name, channel_config) in &config.channels {
        let token_status = if let Some(ref env_var) = channel_config.token_env {
            if std::env::var(env_var).is_ok() {
                "set"
            } else {
                "NOT SET"
            }
        } else {
            "no token configured"
        };

        let default_fighter = channel_config
            .settings
            .get("default_fighter")
            .and_then(|v| v.as_str())
            .unwrap_or("(none)");

        println!("  {} ({})", name, channel_config.channel_type);
        println!("    Token:           {}", token_status);
        println!("    Default Fighter: {}", default_fighter);

        let webhook_path = match channel_config.channel_type.as_str() {
            "telegram" => "/api/channels/telegram/webhook",
            "discord" => "/api/channels/discord/webhook",
            "slack" => "/api/channels/slack/events",
            _ => "(unknown)",
        };
        println!("    Webhook:         POST {}", webhook_path);
        println!();
    }

    0
}

async fn run_test(platform: &str, config_path: Option<String>) -> i32 {
    let config = match super::load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    let base_url = format!("http://{}", config.api_listen);
    let client = reqwest::Client::new();

    let (endpoint, test_payload) = match platform {
        "telegram" => (
            format!("{}/api/channels/telegram/webhook", base_url),
            serde_json::json!({
                "update_id": 999999,
                "message": {
                    "message_id": 1,
                    "from": { "id": 12345, "first_name": "Test", "last_name": "User" },
                    "chat": { "id": 12345, "type": "private" },
                    "date": chrono::Utc::now().timestamp(),
                    "text": "Hello from punch channel test!"
                }
            }),
        ),
        "discord" => (
            format!("{}/api/channels/discord/webhook", base_url),
            serde_json::json!({
                "id": "test_msg_1",
                "channel_id": "test_channel",
                "content": "Hello from punch channel test!",
                "author": {
                    "id": "test_user_1",
                    "username": "test_user",
                    "bot": false
                },
                "timestamp": chrono::Utc::now().to_rfc3339()
            }),
        ),
        "slack" => (
            format!("{}/api/channels/slack/events", base_url),
            serde_json::json!({
                "type": "event_callback",
                "event": {
                    "type": "message",
                    "user": "U_TEST_USER",
                    "channel": "C_TEST_CHANNEL",
                    "text": "Hello from punch channel test!",
                    "ts": format!("{}.000100", chrono::Utc::now().timestamp())
                }
            }),
        ),
        _ => {
            eprintln!("  [X] Unknown platform: {}. Use: telegram, discord, slack", platform);
            return 1;
        }
    };

    println!("  Testing {} channel...", platform);
    println!("  Endpoint: POST {}", endpoint);
    println!();

    match client.post(&endpoint).json(&test_payload).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_default();

            if status.is_success() {
                println!("  Status: {} OK", status.as_u16());
                if let Some(response) = body["response"].as_str() {
                    println!("  Response: {}", response);
                }
                if let Some(error) = body["error"].as_str() {
                    println!("  Note: {}", error);
                }
            } else {
                println!("  Status: {} FAILED", status.as_u16());
                println!("  Body: {}", serde_json::to_string_pretty(&body).unwrap_or_default());
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to connect: {}", e);
            eprintln!("  Make sure the daemon is running (`punch start`).");
            return 1;
        }
    }

    0
}
