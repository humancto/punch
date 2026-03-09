//! `punch gorilla` — Manage gorillas (autonomous agents).

use std::sync::Arc;

use super::{load_config, load_dotenv, punch_home};
use crate::cli::GorillaCommands;

pub async fn run(command: GorillaCommands, config_path: Option<String>) -> i32 {
    load_dotenv();

    match command {
        GorillaCommands::List => run_list(config_path).await,
        GorillaCommands::Unleash { name } => run_unleash(name, config_path).await,
        GorillaCommands::Cage { name } => run_cage(name, config_path).await,
        GorillaCommands::Status { name } => run_status(name, config_path).await,
        GorillaCommands::Test { name } => run_test(name, config_path).await,
    }
}

/// Try to read the daemon port from config. Returns the base URL if daemon is reachable.
fn daemon_url(config_path: Option<&str>) -> Option<String> {
    let config = load_config(config_path).ok()?;
    let url = format!("http://{}", config.api_listen);
    let health_url = format!("{}/health", url);

    let url_clone = url.clone();
    let handle = std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .ok()?;
        client.get(&health_url).send().ok().and_then(|resp| {
            if resp.status().is_success() {
                Some(url_clone)
            } else {
                None
            }
        })
    });
    handle.join().ok().flatten()
}

async fn run_list(config_path: Option<String>) -> i32 {
    // Try daemon first.
    if let Some(base_url) = daemon_url(config_path.as_deref()) {
        let client = reqwest::Client::new();
        let url = format!("{}/api/gorillas", base_url);

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(gorillas) = resp.json::<Vec<serde_json::Value>>().await {
                    if gorillas.is_empty() {
                        println!();
                        println!("  No gorillas registered.");
                        println!(
                            "  Gorillas are loaded from bundled manifests when the daemon starts."
                        );
                        println!();
                        return 0;
                    }

                    println!();
                    println!("  {:<24}  {:<12}  SCHEDULE", "NAME", "STATUS");
                    println!("  {}", "-".repeat(70));

                    for g in &gorillas {
                        println!(
                            "  {:<24}  {:<12}  {}",
                            g["name"].as_str().unwrap_or("-"),
                            g["status"].as_str().unwrap_or("-"),
                            g["schedule"].as_str().unwrap_or("-"),
                        );
                    }

                    println!();
                    println!("  Total: {} gorilla(s) (via daemon)", gorillas.len());
                    println!();
                    return 0;
                }
            }
            _ => {}
        }
    }

    // No daemon running.
    println!();
    println!("  Daemon is not running. Start it with: punch start");
    println!("  Gorillas are registered when the daemon starts.");
    println!();
    1
}

async fn run_unleash(name: String, config_path: Option<String>) -> i32 {
    // Must go through daemon.
    let base_url = match daemon_url(config_path.as_deref()) {
        Some(url) => url,
        None => {
            eprintln!("  [X] Daemon is not running. Start it with: punch start");
            return 1;
        }
    };

    // First, list gorillas to find the one by name.
    let client = reqwest::Client::new();
    let list_url = format!("{}/api/gorillas", base_url);

    let gorillas = match client.get(&list_url).send().await {
        Ok(resp) if resp.status().is_success() => resp
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap_or_default(),
        _ => {
            eprintln!("  [X] Failed to list gorillas from daemon.");
            return 1;
        }
    };

    let gorilla = gorillas.iter().find(|g| {
        g["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(&name))
    });

    match gorilla {
        Some(g) => {
            let id = g["id"].as_str().unwrap_or("");
            let unleash_url = format!("{}/api/gorillas/{}/unleash", base_url, id);

            match client.post(&unleash_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let gorilla_name = g["name"].as_str().unwrap_or(&name);
                    let schedule = g["schedule"].as_str().unwrap_or("unknown");
                    println!();
                    println!("  {} has been UNLEASHED!", gorilla_name);
                    println!("  Schedule: {}", schedule);
                    println!();
                    0
                }
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    eprintln!("  [X] Failed to unleash gorilla: {}", body);
                    1
                }
                Err(e) => {
                    eprintln!("  [X] Failed to reach daemon: {}", e);
                    1
                }
            }
        }
        None => {
            eprintln!("  [X] Gorilla '{}' not found.", name);
            eprintln!("  Run `punch gorilla list` to see available gorillas.");
            1
        }
    }
}

async fn run_cage(name: String, config_path: Option<String>) -> i32 {
    let base_url = match daemon_url(config_path.as_deref()) {
        Some(url) => url,
        None => {
            eprintln!("  [X] Daemon is not running. Start it with: punch start");
            return 1;
        }
    };

    let client = reqwest::Client::new();
    let list_url = format!("{}/api/gorillas", base_url);

    let gorillas = match client.get(&list_url).send().await {
        Ok(resp) if resp.status().is_success() => resp
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap_or_default(),
        _ => {
            eprintln!("  [X] Failed to list gorillas from daemon.");
            return 1;
        }
    };

    let gorilla = gorillas.iter().find(|g| {
        g["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(&name))
    });

    match gorilla {
        Some(g) => {
            let id = g["id"].as_str().unwrap_or("");
            let cage_url = format!("{}/api/gorillas/{}/cage", base_url, id);

            match client.post(&cage_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let gorilla_name = g["name"].as_str().unwrap_or(&name);
                    println!();
                    println!("  {} has been CAGED.", gorilla_name);
                    println!();
                    0
                }
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    eprintln!("  [X] Failed to cage gorilla: {}", body);
                    1
                }
                Err(e) => {
                    eprintln!("  [X] Failed to reach daemon: {}", e);
                    1
                }
            }
        }
        None => {
            eprintln!("  [X] Gorilla '{}' not found.", name);
            1
        }
    }
}

async fn run_status(name: String, config_path: Option<String>) -> i32 {
    let base_url = match daemon_url(config_path.as_deref()) {
        Some(url) => url,
        None => {
            eprintln!("  [X] Daemon is not running. Start it with: punch start");
            return 1;
        }
    };

    let client = reqwest::Client::new();
    let list_url = format!("{}/api/gorillas", base_url);

    let gorillas = match client.get(&list_url).send().await {
        Ok(resp) if resp.status().is_success() => resp
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap_or_default(),
        _ => {
            eprintln!("  [X] Failed to list gorillas from daemon.");
            return 1;
        }
    };

    let gorilla = gorillas.iter().find(|g| {
        g["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(&name))
    });

    match gorilla {
        Some(g) => {
            println!();
            println!("  Gorilla: {}", g["name"].as_str().unwrap_or("-"));
            println!("  Status:  {}", g["status"].as_str().unwrap_or("-"));
            println!("  Schedule: {}", g["schedule"].as_str().unwrap_or("-"));
            println!(
                "  Description: {}",
                g["description"].as_str().unwrap_or("-")
            );
            println!();
            0
        }
        None => {
            eprintln!("  [X] Gorilla '{}' not found.", name);
            1
        }
    }
}

/// Run a single test tick of a gorilla locally (without the daemon).
///
/// This loads the gorilla manifest from bundled or user gorillas, creates a
/// temporary Ring, and runs a single autonomous tick to verify the config works.
async fn run_test(name: String, config_path: Option<String>) -> i32 {
    let config = match load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    // Find the gorilla manifest by name.
    let manifest = match find_gorilla_manifest(&name) {
        Some(m) => m,
        None => {
            eprintln!("  [X] Gorilla '{}' not found in bundled or user gorillas.", name);
            eprintln!("  Searched in:");
            eprintln!("    - crates/punch-gorillas/bundled/");
            eprintln!("    - ~/.punch/gorillas/");
            return 1;
        }
    };

    println!();
    println!("  Testing gorilla: {}", manifest.name);
    println!("  Schedule: {}", manifest.schedule);
    println!("  Capabilities: {:?}", manifest.effective_capabilities());

    let effective_model = manifest.model.clone().unwrap_or_else(|| config.default_model.clone());
    println!("  Model: {} ({})", effective_model.model, effective_model.provider);
    println!();
    println!("  Running single autonomous tick...");
    println!();

    // Create a temporary in-memory memory substrate.
    let memory = match punch_memory::MemorySubstrate::in_memory() {
        Ok(m) => Arc::new(m),
        Err(e) => {
            eprintln!("  [X] Failed to create memory substrate: {}", e);
            return 1;
        }
    };

    // Create the LLM driver using the gorilla's model or the default.
    let driver = match punch_runtime::create_driver(&effective_model) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  [X] Failed to create LLM driver: {}", e);
            return 1;
        }
    };

    let gorilla_id = punch_types::GorillaId::new();

    match punch_kernel::run_gorilla_tick(
        gorilla_id,
        &manifest,
        &config.default_model,
        &memory,
        &driver,
    )
    .await
    {
        Ok(result) => {
            println!("  --- Gorilla Response ---");
            println!();
            // Indent the response for readability.
            for line in result.response.lines() {
                println!("  {}", line);
            }
            println!();
            println!("  ---");
            println!("  Iterations:  {}", result.iterations);
            println!("  Tool calls:  {}", result.tool_calls_made);
            println!("  Tokens used: {}", result.usage.total());
            println!();
            println!("  Test PASSED. Gorilla '{}' is working.", name);
            println!();
            0
        }
        Err(e) => {
            eprintln!("  [X] Gorilla tick failed: {}", e);
            eprintln!();
            eprintln!("  Test FAILED. Check your model configuration and API keys.");
            1
        }
    }
}

/// Search for a gorilla manifest file by name in bundled and user directories.
fn find_gorilla_manifest(name: &str) -> Option<punch_types::GorillaManifest> {
    let search_paths = vec![
        // Development: relative to project root.
        std::path::PathBuf::from("crates/punch-gorillas/bundled"),
        // User-installed gorillas.
        punch_home().join("gorillas"),
    ];

    for base_dir in &search_paths {
        if !base_dir.exists() {
            continue;
        }

        let entries = match std::fs::read_dir(base_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let toml_path = path.join("GORILLA.toml");
            if !toml_path.exists() {
                continue;
            }

            if let Ok(contents) = std::fs::read_to_string(&toml_path) {
                if let Ok(manifest) = toml::from_str::<punch_types::GorillaManifest>(&contents) {
                    if manifest.name.eq_ignore_ascii_case(name) {
                        return Some(manifest);
                    }
                }
            }
        }
    }

    None
}
