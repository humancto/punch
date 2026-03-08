//! `punch config` — Manage Punch configuration.

use super::config_path;
use crate::cli::ConfigCommands;

pub async fn run(command: ConfigCommands) -> i32 {
    match command {
        ConfigCommands::Show => run_show().await,
        ConfigCommands::Edit => run_edit().await,
        ConfigCommands::Set { key, value } => run_set(key, value).await,
    }
}

async fn run_show() -> i32 {
    let path = config_path(None);

    if !path.exists() {
        eprintln!("  [X] Config file not found at {}", path.display());
        eprintln!("      Run `punch init` first.");
        return 1;
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            println!();
            println!("  # {}", path.display());
            println!("  # {}", "-".repeat(60));
            println!();
            for line in contents.lines() {
                println!("  {}", line);
            }
            println!();
            0
        }
        Err(e) => {
            eprintln!("  [X] Failed to read config: {}", e);
            1
        }
    }
}

async fn run_edit() -> i32 {
    let path = config_path(None);

    if !path.exists() {
        eprintln!("  [X] Config file not found at {}", path.display());
        eprintln!("      Run `punch init` first.");
        return 1;
    }

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    println!("  Opening {} with {}...", path.display(), editor);

    match std::process::Command::new(&editor).arg(&path).status() {
        Ok(status) => {
            if status.success() {
                0
            } else {
                eprintln!("  [X] Editor exited with: {}", status);
                1
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to launch editor '{}': {}", editor, e);
            1
        }
    }
}

async fn run_set(key: String, value: String) -> i32 {
    let path = config_path(None);

    if !path.exists() {
        eprintln!("  [X] Config file not found at {}", path.display());
        eprintln!("      Run `punch init` first.");
        return 1;
    }

    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] Failed to read config: {}", e);
            return 1;
        }
    };

    // Parse as a TOML table for manipulation.
    let mut doc: toml::Table = match contents.parse() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  [X] Failed to parse config: {}", e);
            return 1;
        }
    };

    // Navigate dot-separated key path.
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        eprintln!("  [X] Invalid key: {}", key);
        return 1;
    }

    // Set the value. Try to parse as appropriate TOML type.
    let toml_value = if value == "true" {
        toml::Value::Boolean(true)
    } else if value == "false" {
        toml::Value::Boolean(false)
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else if let Ok(n) = value.parse::<f64>() {
        toml::Value::Float(n)
    } else {
        toml::Value::String(value.clone())
    };

    // Navigate to the right table and set the value.
    if parts.len() == 1 {
        doc.insert(parts[0].to_string(), toml_value);
    } else {
        let mut current = &mut doc;
        for &part in &parts[..parts.len() - 1] {
            current = current
                .entry(part)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .expect("expected table in config path");
        }
        let last_key = parts[parts.len() - 1];
        current.insert(last_key.to_string(), toml_value);
    }

    // Write back.
    let output = toml::to_string_pretty(&doc).expect("failed to serialize config");
    if let Err(e) = std::fs::write(&path, output) {
        eprintln!("  [X] Failed to write config: {}", e);
        return 1;
    }

    println!("  Set {} = {}", key, value);
    0
}
