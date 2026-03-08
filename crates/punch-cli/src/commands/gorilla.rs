//! `punch gorilla` — Manage gorillas (autonomous agents).

use std::sync::Arc;

use punch_kernel::Ring;
use punch_memory::MemorySubstrate;
use punch_runtime::create_driver;
use crate::cli::GorillaCommands;
use super::{load_config, load_dotenv};

pub async fn run(command: GorillaCommands, config_path: Option<String>) -> i32 {
    load_dotenv();

    match command {
        GorillaCommands::List => run_list(config_path).await,
        GorillaCommands::Unleash { name } => run_unleash(name, config_path).await,
        GorillaCommands::Cage { name } => run_cage(name, config_path).await,
        GorillaCommands::Status { name } => run_status(name, config_path).await,
    }
}

async fn run_list(config_path: Option<String>) -> i32 {
    let config = match load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    let ring = match create_ring(&config).await {
        Ok(r) => r,
        Err(code) => return code,
    };

    let gorillas = ring.list_gorillas().await;

    if gorillas.is_empty() {
        println!();
        println!("  No gorillas registered.");
        println!("  Install gorillas from the moves registry or define your own.");
        println!();
        return 0;
    }

    println!();
    println!("  {:<36}  {:<20}  {:<12}  {}", "ID", "NAME", "STATUS", "SCHEDULE");
    println!("  {}", "-".repeat(90));

    for (id, manifest, status) in &gorillas {
        println!(
            "  {:<36}  {:<20}  {:<12}  {}",
            id, manifest.name, status, manifest.schedule
        );
    }

    println!();
    println!("  Total: {} gorilla(s)", gorillas.len());
    println!();

    0
}

async fn run_unleash(name: String, config_path: Option<String>) -> i32 {
    let config = match load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    let ring = match create_ring(&config).await {
        Ok(r) => r,
        Err(code) => return code,
    };

    let gorillas = ring.list_gorillas().await;
    let found = gorillas
        .iter()
        .find(|(_, m, _)| m.name.eq_ignore_ascii_case(&name));

    match found {
        Some((id, manifest, _)) => {
            match ring.unleash_gorilla(id).await {
                Ok(()) => {
                    println!();
                    println!("  {} has been UNLEASHED!", manifest.name);
                    println!("  Schedule: {}", manifest.schedule);
                    println!();
                    0
                }
                Err(e) => {
                    eprintln!("  [X] Failed to unleash gorilla: {}", e);
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

async fn run_cage(name: String, config_path: Option<String>) -> i32 {
    let config = match load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    let ring = match create_ring(&config).await {
        Ok(r) => r,
        Err(code) => return code,
    };

    let gorillas = ring.list_gorillas().await;
    let found = gorillas
        .iter()
        .find(|(_, m, _)| m.name.eq_ignore_ascii_case(&name));

    match found {
        Some((id, manifest, _)) => {
            match ring.cage_gorilla(id).await {
                Ok(()) => {
                    println!();
                    println!("  {} has been CAGED.", manifest.name);
                    println!();
                    0
                }
                Err(e) => {
                    eprintln!("  [X] Failed to cage gorilla: {}", e);
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
    let config = match load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    let ring = match create_ring(&config).await {
        Ok(r) => r,
        Err(code) => return code,
    };

    let gorillas = ring.list_gorillas().await;
    let found = gorillas
        .iter()
        .find(|(_, m, _)| m.name.eq_ignore_ascii_case(&name));

    match found {
        Some((_, manifest, status)) => {
            println!();
            println!("  Gorilla: {}", manifest.name);
            println!("  Status:  {}", status);
            println!("  Schedule: {}", manifest.schedule);
            println!("  Description: {}", manifest.description);
            println!();
            println!("  Required Moves:");
            if manifest.moves_required.is_empty() {
                println!("    (none)");
            } else {
                for m in &manifest.moves_required {
                    println!("    - {}", m);
                }
            }
            println!();
            println!("  Dashboard Metrics:");
            if manifest.dashboard_metrics.is_empty() {
                println!("    (none)");
            } else {
                for m in &manifest.dashboard_metrics {
                    println!("    - {}", m);
                }
            }
            println!();
            0
        }
        None => {
            eprintln!("  [X] Gorilla '{}' not found.", name);
            1
        }
    }
}

/// Create a Ring from config for CLI operations.
async fn create_ring(
    config: &punch_types::PunchConfig,
) -> Result<Arc<Ring>, i32> {
    let db_path_str = if config.memory.db_path.starts_with("~") {
        let home = dirs::home_dir().expect("could not determine home directory");
        config.memory.db_path.replace("~", &home.to_string_lossy())
    } else {
        config.memory.db_path.clone()
    };
    let db_path = std::path::Path::new(&db_path_str);

    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let memory = match MemorySubstrate::new(db_path) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            eprintln!("  [X] Failed to initialize memory: {}", e);
            return Err(1);
        }
    };

    let driver = match create_driver(&config.default_model) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  [X] Failed to create LLM driver: {}", e);
            return Err(1);
        }
    };

    Ok(Arc::new(Ring::new(config.clone(), memory, driver)))
}
