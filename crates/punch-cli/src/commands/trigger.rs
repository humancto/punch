//! `punch trigger` — Manage event-driven triggers.

use crate::cli::TriggerCommands;

/// Base URL for the Punch daemon API.
fn api_base() -> String {
    std::env::var("PUNCH_API_URL").unwrap_or_else(|_| "http://127.0.0.1:6660".to_string())
}

pub async fn run(command: TriggerCommands) -> i32 {
    match command {
        TriggerCommands::List => run_list().await,
        TriggerCommands::Add {
            trigger_type,
            gorilla,
            config,
        } => run_add(&trigger_type, gorilla.as_deref(), &config).await,
        TriggerCommands::Remove { id } => run_remove(&id).await,
        TriggerCommands::Test { id } => run_test(&id).await,
    }
}

async fn run_list() -> i32 {
    let url = format!("{}/api/triggers", api_base());
    let client = reqwest::Client::new();

    match client.get(&url).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(triggers) => {
                        if let Some(arr) = triggers.as_array() {
                            if arr.is_empty() {
                                println!("No triggers registered.");
                            } else {
                                println!(
                                    "{:<38} {:<20} {:<15} {:<8} FIRES",
                                    "ID", "NAME", "TYPE", "ENABLED"
                                );
                                println!("{}", "-".repeat(90));
                                for t in arr {
                                    println!(
                                        "{:<38} {:<20} {:<15} {:<8} {}",
                                        t["id"]["0"].as_str().unwrap_or(&t["id"].to_string()),
                                        t["name"].as_str().unwrap_or("?"),
                                        t["condition_type"].as_str().unwrap_or("?"),
                                        t["enabled"].as_bool().unwrap_or(false),
                                        t["fire_count"].as_u64().unwrap_or(0),
                                    );
                                }
                            }
                        }
                        0
                    }
                    Err(e) => {
                        eprintln!("  [X] Failed to parse response: {}", e);
                        1
                    }
                }
            } else {
                eprintln!("  [X] API error: {}", resp.status());
                1
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to connect to daemon: {}", e);
            eprintln!("      Is the daemon running? Try: punch start");
            1
        }
    }
}

async fn run_add(trigger_type: &str, gorilla: Option<&str>, config_json: &str) -> i32 {
    let url = format!("{}/api/triggers", api_base());
    let client = reqwest::Client::new();

    // Parse the config JSON to extract the trigger parameters.
    let config: serde_json::Value = match serde_json::from_str(config_json) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  [X] Invalid JSON config: {}", e);
            eprintln!(
                "      Example: punch trigger add keyword '{{\"name\":\"deploy-trigger\",\"keywords\":[\"deploy\"],\"action\":{{\"action\":\"log\",\"message\":\"deploy triggered\"}}}}'"
            );
            return 1;
        }
    };

    // Build the condition based on trigger_type.
    let condition = match trigger_type {
        "keyword" => {
            let keywords = config["keywords"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if keywords.is_empty() {
                eprintln!("  [X] keyword trigger requires 'keywords' array");
                return 1;
            }
            serde_json::json!({ "type": "keyword", "keywords": keywords })
        }
        "schedule" => {
            let interval = config["interval_secs"].as_u64().unwrap_or(0);
            if interval == 0 {
                eprintln!("  [X] schedule trigger requires 'interval_secs'");
                return 1;
            }
            serde_json::json!({ "type": "schedule", "interval_secs": interval })
        }
        "event" => {
            let event_kind = config["event_kind"].as_str().unwrap_or("");
            if event_kind.is_empty() {
                eprintln!("  [X] event trigger requires 'event_kind'");
                return 1;
            }
            serde_json::json!({ "type": "event", "event_kind": event_kind })
        }
        "webhook" => {
            let secret = config["secret"].as_str().map(String::from);
            serde_json::json!({ "type": "webhook", "secret": secret })
        }
        _ => {
            eprintln!("  [X] Unknown trigger type: {}", trigger_type);
            eprintln!("      Valid types: keyword, schedule, event, webhook");
            return 1;
        }
    };

    let name = config["name"]
        .as_str()
        .unwrap_or("unnamed-trigger")
        .to_string();

    let action = if let Some(action_val) = config.get("action") {
        action_val.clone()
    } else {
        serde_json::json!({ "action": "log", "message": "trigger fired" })
    };

    let max_fires = config["max_fires"].as_u64().unwrap_or(0);

    let mut body = serde_json::json!({
        "name": name,
        "condition": condition,
        "action": action,
        "max_fires": max_fires,
    });

    if let Some(gorilla_name) = gorilla {
        body["gorilla"] = serde_json::Value::String(gorilla_name.to_string());
    }

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            if resp.status().is_success() || resp.status() == reqwest::StatusCode::CREATED {
                match resp.json::<serde_json::Value>().await {
                    Ok(result) => {
                        println!("Trigger registered:");
                        println!(
                            "  ID:   {}",
                            result["id"]["0"]
                                .as_str()
                                .unwrap_or(&result["id"].to_string())
                        );
                        println!("  Name: {}", result["name"].as_str().unwrap_or("?"));
                        0
                    }
                    Err(e) => {
                        eprintln!("  [X] Failed to parse response: {}", e);
                        1
                    }
                }
            } else {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                eprintln!("  [X] API error ({}): {}", status, body);
                1
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to connect to daemon: {}", e);
            1
        }
    }
}

async fn run_remove(id: &str) -> i32 {
    let url = format!("{}/api/triggers/{}", api_base(), id);
    let client = reqwest::Client::new();

    match client.delete(&url).send().await {
        Ok(resp) => {
            if resp.status().is_success() || resp.status() == reqwest::StatusCode::NO_CONTENT {
                println!("Trigger {} removed.", id);
                0
            } else {
                eprintln!("  [X] API error: {}", resp.status());
                1
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to connect to daemon: {}", e);
            1
        }
    }
}

async fn run_test(id: &str) -> i32 {
    let url = format!("{}/api/triggers/{}/test", api_base(), id);
    let client = reqwest::Client::new();

    println!("  Testing trigger {}...", id);

    match client.post(&url).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(result) => {
                        println!();
                        println!("  Trigger test result:");
                        let fired = result["fired"].as_bool().unwrap_or(false);
                        let status = if fired { "FIRED" } else { "DID NOT FIRE" };
                        println!("  Status:  {}", status);
                        if let Some(msg) = result["message"].as_str() {
                            println!("  Message: {}", msg);
                        }
                        if let Some(action) = result["action_result"].as_str() {
                            println!("  Action:  {}", action);
                        }
                        println!();
                        0
                    }
                    Err(e) => {
                        eprintln!("  [X] Failed to parse response: {}", e);
                        1
                    }
                }
            } else if resp.status() == reqwest::StatusCode::NOT_FOUND {
                eprintln!("  [X] Trigger '{}' not found.", id);
                1
            } else {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                eprintln!("  [X] API error ({}): {}", status, body);
                1
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to connect to daemon: {}", e);
            eprintln!("      Is the daemon running? Try: punch start");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    /// Build the triggers list URL.
    fn build_list_url(base: &str) -> String {
        format!("{}/api/triggers", base)
    }

    /// Build the trigger add URL.
    fn build_add_url(base: &str) -> String {
        format!("{}/api/triggers", base)
    }

    /// Build the trigger remove URL.
    fn build_remove_url(base: &str, id: &str) -> String {
        format!("{}/api/triggers/{}", base, id)
    }

    /// Build the trigger test URL.
    fn build_test_url(base: &str, id: &str) -> String {
        format!("{}/api/triggers/{}/test", base, id)
    }

    /// Format a trigger list as a table.
    fn format_triggers_table(triggers: &[serde_json::Value]) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "{:<38} {:<20} {:<15} {:<8} FIRES",
            "ID", "NAME", "TYPE", "ENABLED"
        ));
        lines.push("-".repeat(90));

        for t in triggers {
            lines.push(format!(
                "{:<38} {:<20} {:<15} {:<8} {}",
                t["id"]["0"].as_str().unwrap_or(&t["id"].to_string()),
                t["name"].as_str().unwrap_or("?"),
                t["condition_type"].as_str().unwrap_or("?"),
                t["enabled"].as_bool().unwrap_or(false),
                t["fire_count"].as_u64().unwrap_or(0),
            ));
        }

        lines.join("\n")
    }

    /// Format error message for daemon connection failure.
    fn format_connection_error() -> String {
        "Failed to connect to daemon. Is the daemon running? Try: punch start".to_string()
    }

    #[test]
    fn test_build_list_url() {
        assert_eq!(
            build_list_url("http://localhost:6660"),
            "http://localhost:6660/api/triggers"
        );
    }

    #[test]
    fn test_build_add_url() {
        assert_eq!(
            build_add_url("http://localhost:6660"),
            "http://localhost:6660/api/triggers"
        );
    }

    #[test]
    fn test_build_remove_url() {
        assert_eq!(
            build_remove_url("http://localhost:6660", "abc-123"),
            "http://localhost:6660/api/triggers/abc-123"
        );
    }

    #[test]
    fn test_build_test_url() {
        assert_eq!(
            build_test_url("http://localhost:6660", "abc-123"),
            "http://localhost:6660/api/triggers/abc-123/test"
        );
    }

    #[test]
    fn test_format_triggers_table_empty() {
        let table = format_triggers_table(&[]);
        assert!(table.contains("ID"));
        assert!(table.contains("NAME"));
    }

    #[test]
    fn test_format_triggers_table_with_data() {
        let triggers = vec![serde_json::json!({
            "id": {"0": "abc-123"},
            "name": "deploy-trigger",
            "condition_type": "keyword",
            "enabled": true,
            "fire_count": 5,
        })];
        let table = format_triggers_table(&triggers);
        assert!(table.contains("deploy-trigger"));
        assert!(table.contains("keyword"));
    }

    #[test]
    fn test_api_base_default() {
        let url = build_list_url("http://127.0.0.1:6660");
        assert_eq!(url, "http://127.0.0.1:6660/api/triggers");
    }

    #[test]
    fn test_format_connection_error() {
        let msg = format_connection_error();
        assert!(msg.contains("daemon"));
        assert!(msg.contains("punch start"));
    }

    #[test]
    fn test_trigger_condition_json_keyword() {
        let condition = serde_json::json!({ "type": "keyword", "keywords": ["deploy"] });
        assert_eq!(condition["type"], "keyword");
        assert!(condition["keywords"].as_array().is_some());
    }

    #[test]
    fn test_trigger_condition_json_schedule() {
        let condition = serde_json::json!({ "type": "schedule", "interval_secs": 300 });
        assert_eq!(condition["type"], "schedule");
        assert_eq!(condition["interval_secs"], 300);
    }

    #[test]
    fn test_trigger_body_with_gorilla() {
        let mut body = serde_json::json!({
            "name": "test",
            "condition": {},
            "action": {},
            "max_fires": 0,
        });
        body["gorilla"] = serde_json::Value::String("alpha".to_string());
        assert_eq!(body["gorilla"], "alpha");
    }

    #[test]
    fn test_trigger_test_response_parsing() {
        let result = serde_json::json!({
            "fired": true,
            "message": "Trigger fired successfully",
            "action_result": "log: test complete"
        });
        assert!(result["fired"].as_bool().unwrap());
        assert_eq!(
            result["message"].as_str().unwrap(),
            "Trigger fired successfully"
        );
    }
}
