//! `punch fighter` — Manage fighters (conversational agents).

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use punch_kernel::Ring;
use punch_memory::MemorySubstrate;
use punch_runtime::create_driver;
use punch_types::{FighterId, FighterManifest, WeightClass};
use punch_types::config::ModelConfig;
use uuid::Uuid;

use crate::cli::FighterCommands;
use super::{load_config, load_dotenv, punch_home};

pub async fn run(command: FighterCommands, config_path: Option<String>) -> i32 {
    load_dotenv();

    match command {
        FighterCommands::Spawn { template } => run_spawn(template, config_path).await,
        FighterCommands::List => run_list(config_path).await,
        FighterCommands::Chat { name } => run_chat(name, config_path).await,
        FighterCommands::Send { id, message } => run_send(id, message, config_path).await,
        FighterCommands::Kill { id } => run_kill(id, config_path).await,
    }
}

/// Quick chat entry point (from `punch chat`).
pub async fn run_quick_chat(message: Option<String>, config_path: Option<String>) -> i32 {
    load_dotenv();

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

    // Spawn a default fighter for quick chat.
    let manifest = FighterManifest {
        name: "Punch".to_string(),
        description: "Default Punch fighter for quick chat.".to_string(),
        model: config.default_model.clone(),
        system_prompt: "You are Punch, a helpful AI assistant with the heart of a champion. \
            Be concise, direct, and helpful."
            .to_string(),
        capabilities: vec![],
        weight_class: WeightClass::Middleweight,
    };

    let fighter_id = ring.spawn_fighter(manifest).await;

    match message {
        Some(msg) => {
            // One-shot mode.
            match ring.send_message(&fighter_id, msg).await {
                Ok(result) => {
                    println!("{}", result.response);
                    0
                }
                Err(e) => {
                    eprintln!("  [X] {}", e);
                    1
                }
            }
        }
        None => {
            // Interactive REPL.
            run_chat_repl(&ring, &fighter_id).await
        }
    }
}

async fn run_spawn(template: String, config_path: Option<String>) -> i32 {
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

    // Load template from ~/.punch/fighters/<template>.toml or use built-in.
    let manifest = match load_template(&template, &config.default_model) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    let name = manifest.name.clone();
    let id = ring.spawn_fighter(manifest).await;

    println!();
    println!("  Fighter spawned!");
    println!("  Name:  {}", name);
    println!("  ID:    {}", id);
    println!("  Class: middleweight");
    println!();
    println!("  Start chatting: punch fighter chat {}", name.to_lowercase());
    println!();

    0
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

    let fighters = ring.list_fighters();

    if fighters.is_empty() {
        println!();
        println!("  No fighters in the ring.");
        println!("  Spawn one: punch fighter spawn <template>");
        println!();
        return 0;
    }

    println!();
    println!("  {:<36}  {:<16}  {:<14}  {}", "ID", "NAME", "WEIGHT CLASS", "STATUS");
    println!("  {}", "-".repeat(86));

    for (id, manifest, status) in &fighters {
        println!(
            "  {:<36}  {:<16}  {:<14}  {}",
            id, manifest.name, manifest.weight_class, status
        );
    }

    println!();
    println!("  Total: {} fighter(s)", fighters.len());
    println!();

    0
}

async fn run_chat(name: Option<String>, config_path: Option<String>) -> i32 {
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

    // If a name is given, look up the fighter; otherwise spawn a default.
    let fighter_id = match name {
        Some(ref n) => {
            let fighters = ring.list_fighters();
            match fighters.iter().find(|(_, m, _)| m.name.eq_ignore_ascii_case(n)) {
                Some((id, _, _)) => *id,
                None => {
                    eprintln!("  [X] Fighter '{}' not found.", n);
                    return 1;
                }
            }
        }
        None => {
            let manifest = FighterManifest {
                name: "Punch".to_string(),
                description: "Default Punch fighter.".to_string(),
                model: config.default_model.clone(),
                system_prompt: "You are Punch, a helpful AI assistant with the heart of a champion. \
                    Be concise, direct, and helpful."
                    .to_string(),
                capabilities: vec![],
                weight_class: WeightClass::Middleweight,
            };
            ring.spawn_fighter(manifest).await
        }
    };

    run_chat_repl(&ring, &fighter_id).await
}

async fn run_chat_repl(ring: &Arc<Ring>, fighter_id: &FighterId) -> i32 {
    let fighter = ring.get_fighter(fighter_id);
    let fighter_name = fighter
        .map(|f| f.manifest.name.clone())
        .unwrap_or_else(|| "Fighter".to_string());

    println!();
    println!("  Entering the ring with {}...", fighter_name);
    println!("  Type your message and press Enter. Type /exit to leave.");
    println!();

    let stdin = io::stdin();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let input = line.trim().to_string();

        if input.is_empty() {
            continue;
        }

        if input == "/exit" || input == "/quit" || input == "/q" {
            println!("  Bell rings. Fight over.");
            break;
        }

        print!("  {} > ", fighter_name);
        io::stdout().flush().unwrap();

        match ring.send_message(fighter_id, input).await {
            Ok(result) => {
                println!("{}", result.response);
                println!();
            }
            Err(e) => {
                eprintln!("  [X] Error: {}", e);
                println!();
            }
        }

        print!("  you > ");
        io::stdout().flush().unwrap();
    }

    0
}

async fn run_send(id: String, message: String, config_path: Option<String>) -> i32 {
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

    let fighter_id = match Uuid::parse_str(&id) {
        Ok(uuid) => FighterId(uuid),
        Err(_) => {
            eprintln!("  [X] Invalid fighter ID: {}", id);
            return 1;
        }
    };

    match ring.send_message(&fighter_id, message).await {
        Ok(result) => {
            println!("{}", result.response);
            0
        }
        Err(e) => {
            eprintln!("  [X] {}", e);
            1
        }
    }
}

async fn run_kill(id: String, config_path: Option<String>) -> i32 {
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

    let fighter_id = match Uuid::parse_str(&id) {
        Ok(uuid) => FighterId(uuid),
        Err(_) => {
            eprintln!("  [X] Invalid fighter ID: {}", id);
            return 1;
        }
    };

    ring.kill_fighter(&fighter_id);
    println!("  Fighter {} has been knocked out.", id);
    0
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

/// Load a fighter template from disk or use a built-in default.
fn load_template(template: &str, default_model: &ModelConfig) -> Result<FighterManifest, String> {
    // Check for a template file first.
    let template_path = punch_home().join("fighters").join(format!("{}.toml", template));

    if template_path.exists() {
        let contents = std::fs::read_to_string(&template_path)
            .map_err(|e| format!("Failed to read template: {}", e))?;
        let manifest: FighterManifest =
            toml::from_str(&contents).map_err(|e| format!("Failed to parse template: {}", e))?;
        return Ok(manifest);
    }

    // Built-in templates.
    let manifest = match template {
        "default" | "punch" => FighterManifest {
            name: "Punch".to_string(),
            description: "The default all-rounder fighter.".to_string(),
            model: default_model.clone(),
            system_prompt: "You are Punch, a capable AI assistant. Be helpful, concise, and direct."
                .to_string(),
            capabilities: vec![],
            weight_class: WeightClass::Middleweight,
        },
        "coder" => FighterManifest {
            name: "Coder".to_string(),
            description: "A heavyweight coding specialist.".to_string(),
            model: default_model.clone(),
            system_prompt:
                "You are Coder, a programming expert. Write clean, efficient code. \
                 Explain your reasoning. Always include error handling."
                    .to_string(),
            capabilities: vec![],
            weight_class: WeightClass::Heavyweight,
        },
        "scout" => FighterManifest {
            name: "Scout".to_string(),
            description: "A fast featherweight for quick tasks.".to_string(),
            model: default_model.clone(),
            system_prompt:
                "You are Scout, a fast and lightweight assistant. Give brief, \
                 to-the-point answers. Prioritize speed over depth."
                    .to_string(),
            capabilities: vec![],
            weight_class: WeightClass::Featherweight,
        },
        _ => {
            return Err(format!(
                "Unknown template '{}'. Available: default, coder, scout\n  \
                 Or create a template at: {}",
                template,
                template_path.display()
            ));
        }
    };

    Ok(manifest)
}
