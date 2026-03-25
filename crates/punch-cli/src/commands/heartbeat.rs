//! `punch heartbeat` — Manage proactive heartbeat tasks.

use crate::cli::HeartbeatCommands;

/// Base URL for the Punch daemon API.
fn api_base() -> String {
    std::env::var("PUNCH_API_URL").unwrap_or_else(|_| "http://127.0.0.1:6660".to_string())
}

pub async fn run(command: HeartbeatCommands) -> i32 {
    match command {
        HeartbeatCommands::List => run_list().await,
        HeartbeatCommands::Add {
            task,
            cadence,
            fighter,
        } => run_add(&task, &cadence, fighter.as_deref()).await,
        HeartbeatCommands::Remove { index, fighter } => run_remove(index, fighter.as_deref()).await,
    }
}

async fn run_list() -> i32 {
    let url = format!("{}/api/heartbeats", api_base());
    let client = reqwest::Client::new();

    match client.get(&url).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(data) => {
                        if let Some(fighters) = data.as_array() {
                            if fighters.is_empty() {
                                println!("No heartbeat tasks configured.");
                                return 0;
                            }
                            for entry in fighters {
                                let name = entry["fighter_name"].as_str().unwrap_or("?");
                                let tasks = entry["heartbeat"].as_array();
                                if let Some(tasks) = tasks {
                                    if tasks.is_empty() {
                                        continue;
                                    }
                                    println!("\n  Fighter: {}", name);
                                    println!(
                                        "  {:<4} {:<10} {:<8} {:<6} TASK",
                                        "IDX", "CADENCE", "ACTIVE", "RUNS"
                                    );
                                    println!("  {}", "-".repeat(70));
                                    for (i, t) in tasks.iter().enumerate() {
                                        println!(
                                            "  {:<4} {:<10} {:<8} {:<6} {}",
                                            i,
                                            t["cadence"].as_str().unwrap_or("?"),
                                            t["active"].as_bool().unwrap_or(false),
                                            t["execution_count"].as_u64().unwrap_or(0),
                                            t["task"].as_str().unwrap_or("?"),
                                        );
                                    }
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

async fn run_add(task: &str, cadence: &str, fighter: Option<&str>) -> i32 {
    // Validate cadence
    let valid_cadences = ["every_bout", "hourly", "daily", "on_wake"];
    if !valid_cadences.contains(&cadence) {
        eprintln!(
            "  [X] Invalid cadence '{}'. Must be one of: {}",
            cadence,
            valid_cadences.join(", ")
        );
        return 1;
    }

    let url = format!("{}/api/heartbeats", api_base());
    let client = reqwest::Client::new();

    let mut body = serde_json::json!({
        "task": task,
        "cadence": cadence,
    });
    if let Some(name) = fighter {
        body["fighter_name"] = serde_json::json!(name);
    }

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                println!("  Heartbeat added: \"{}\" (cadence: {})", task, cadence);
                0
            } else {
                let msg = resp.text().await.unwrap_or_default();
                eprintln!("  [X] API error: {}", msg);
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

async fn run_remove(index: usize, fighter: Option<&str>) -> i32 {
    let url = format!("{}/api/heartbeats/{}", api_base(), index);
    let client = reqwest::Client::new();

    let mut req = client.delete(&url);
    if let Some(name) = fighter {
        req = req.query(&[("fighter_name", name)]);
    }

    match req.send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                println!("  Heartbeat task {} removed.", index);
                0
            } else {
                let msg = resp.text().await.unwrap_or_default();
                eprintln!("  [X] API error: {}", msg);
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
