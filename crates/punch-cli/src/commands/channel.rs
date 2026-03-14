//! `punch channel` — Manage channel adapters.

use crate::cli::ChannelCommands;

/// Base URL for the Punch daemon API.
fn api_base() -> String {
    std::env::var("PUNCH_API_URL").unwrap_or_else(|_| "http://127.0.0.1:6660".to_string())
}

pub async fn run(command: ChannelCommands, config_path: Option<String>) -> i32 {
    match command {
        ChannelCommands::List => run_list(config_path).await,
        ChannelCommands::Test { platform } => run_test(&platform, config_path).await,
        ChannelCommands::Status { name } => run_status(&name).await,
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
            eprintln!(
                "  [X] Unknown platform: {}. Use: telegram, discord, slack",
                platform
            );
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
                println!(
                    "  Body: {}",
                    serde_json::to_string_pretty(&body).unwrap_or_default()
                );
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

async fn run_status(name: &str) -> i32 {
    let url = format!("{}/api/channels/{}", api_base(), name);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] Failed to create HTTP client: {}", e);
            return 1;
        }
    };

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(data) => {
                    println!();
                    println!(
                        "  Channel: {}",
                        data["name"].as_str().unwrap_or(name)
                    );
                    println!(
                        "  Type:    {}",
                        data["channel_type"].as_str().unwrap_or("-")
                    );
                    println!(
                        "  Status:  {}",
                        data["status"].as_str().unwrap_or("unknown")
                    );

                    if let Some(connected) = data["connected"].as_bool() {
                        println!(
                            "  Connected: {}",
                            if connected { "yes" } else { "no" }
                        );
                    }

                    if let Some(last_msg) = data["last_message_at"].as_str() {
                        println!("  Last message: {}", last_msg);
                    }

                    if let Some(msg_count) = data["message_count"].as_u64() {
                        println!("  Messages: {}", msg_count);
                    }

                    if let Some(fighter) = data["default_fighter"].as_str() {
                        println!("  Default Fighter: {}", fighter);
                    }

                    println!();
                    0
                }
                Err(e) => {
                    eprintln!("  [X] Failed to parse response: {}", e);
                    1
                }
            }
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
            eprintln!("  [X] Channel '{}' not found.", name);
            eprintln!("  Run `punch channel list` to see configured channels.");
            1
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("  [X] API error ({}): {}", status, body);
            1
        }
        Err(e) => {
            if e.is_connect() {
                eprintln!("  [X] Cannot connect to Punch daemon at {}", api_base());
                eprintln!("      Is the daemon running? Try: punch start");
            } else {
                eprintln!("  [X] Request failed: {}", e);
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    /// Build the channels list URL.
    fn build_list_url(base: &str) -> String {
        format!("{}/api/channels", base)
    }

    /// Build the channel test URL.
    fn build_test_url(base: &str, platform: &str) -> String {
        match platform {
            "telegram" => format!("{}/api/channels/telegram/webhook", base),
            "discord" => format!("{}/api/channels/discord/webhook", base),
            "slack" => format!("{}/api/channels/slack/events", base),
            _ => format!("{}/api/channels/{}/test", base, platform),
        }
    }

    /// Build the channel status URL.
    fn build_status_url(base: &str, name: &str) -> String {
        format!("{}/api/channels/{}", base, name)
    }

    /// Format channel status for display.
    fn format_channel_status(data: &serde_json::Value) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "  Channel: {}",
            data["name"].as_str().unwrap_or("-")
        ));
        lines.push(format!(
            "  Type:    {}",
            data["channel_type"].as_str().unwrap_or("-")
        ));
        lines.push(format!(
            "  Status:  {}",
            data["status"].as_str().unwrap_or("unknown")
        ));
        lines.join("\n")
    }

    #[test]
    fn test_build_list_url() {
        assert_eq!(
            build_list_url("http://localhost:6660"),
            "http://localhost:6660/api/channels"
        );
    }

    #[test]
    fn test_build_test_url_telegram() {
        assert_eq!(
            build_test_url("http://localhost:6660", "telegram"),
            "http://localhost:6660/api/channels/telegram/webhook"
        );
    }

    #[test]
    fn test_build_test_url_discord() {
        assert_eq!(
            build_test_url("http://localhost:6660", "discord"),
            "http://localhost:6660/api/channels/discord/webhook"
        );
    }

    #[test]
    fn test_build_test_url_slack() {
        assert_eq!(
            build_test_url("http://localhost:6660", "slack"),
            "http://localhost:6660/api/channels/slack/events"
        );
    }

    #[test]
    fn test_build_status_url() {
        assert_eq!(
            build_status_url("http://localhost:6660", "telegram"),
            "http://localhost:6660/api/channels/telegram"
        );
    }

    #[test]
    fn test_format_channel_status() {
        let data = serde_json::json!({
            "name": "telegram",
            "channel_type": "telegram",
            "status": "connected",
        });
        let output = format_channel_status(&data);
        assert!(output.contains("telegram"));
        assert!(output.contains("connected"));
    }

    #[test]
    fn test_format_channel_status_missing_fields() {
        let data = serde_json::json!({});
        let output = format_channel_status(&data);
        assert!(output.contains("-"));
        assert!(output.contains("unknown"));
    }

    #[test]
    fn test_channel_status_response_parsing() {
        let data = serde_json::json!({
            "name": "discord",
            "channel_type": "discord",
            "status": "active",
            "connected": true,
            "message_count": 42,
            "default_fighter": "oracle",
        });
        assert_eq!(data["name"].as_str().unwrap(), "discord");
        assert!(data["connected"].as_bool().unwrap());
        assert_eq!(data["message_count"].as_u64().unwrap(), 42);
        assert_eq!(data["default_fighter"].as_str().unwrap(), "oracle");
    }

    #[test]
    fn test_api_base_default() {
        let url = build_status_url("http://127.0.0.1:6660", "slack");
        assert_eq!(url, "http://127.0.0.1:6660/api/channels/slack");
    }
}
