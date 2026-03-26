//! `punch start` — Start the Punch daemon (Ring + Arena).

use std::sync::Arc;

use tracing::info;

use punch_api::server::start_arena;
use punch_kernel::{OnError, Ring, Workflow, WorkflowId, WorkflowStep};
use punch_memory::MemorySubstrate;
use punch_runtime::create_driver;
use punch_types::{Capability, GorillaManifest, WeightClass};

use super::{load_config, load_dotenv, punch_home};

const STARTUP_BANNER: &str = r#"
   ______________________________________________________
  |                                                      |
  |    ____  __  ___   ________ __                       |
  |   / __ \/ / / / | / / ____// /  / /                  |
  |  / /_/ / / / /  |/ / /    / /__/ /                   |
  | / ____/ /_/ / /|  / /___ / __  /                     |
  |/_/    \____/_/ |_/\____//_/ /_/                      |
  |                                                      |
  |          THE AGENT COMBAT SYSTEM                     |
  |______________________________________________________|

         ,╓╗╗╗╗╗╗╗╗╗╗╗╗╗╗,
       ╔╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╗
      ╔╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╗
      ╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬     THE RING IS LIVE.
      ╬╬╬╬  ╬╬╬╬╬╬╬╬╬╬  ╬╬╬╬╬     THE ARENA IS OPEN.
      ╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬
       ╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬
        ╚╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬╝
          ╚╬╬╬╬╬╬╬╬╬╬╬╬╬╝
"#;

pub async fn run(config_path: Option<String>, port_override: Option<u16>) -> i32 {
    load_dotenv();

    let mut config = match load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    // Apply port override.
    if let Some(port) = port_override {
        config.api_listen = format!("127.0.0.1:{}", port);
    }

    // Resolve memory DB path.
    let db_path_str = if config.memory.db_path.starts_with("~") {
        let home = dirs::home_dir().expect("could not determine home directory");
        config.memory.db_path.replace("~", &home.to_string_lossy())
    } else {
        config.memory.db_path.clone()
    };
    let db_path = std::path::Path::new(&db_path_str);

    // Ensure the parent directory exists.
    if let Some(parent) = db_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("  [X] Failed to create data directory: {}", e);
        return 1;
    }

    // Initialize the memory substrate.
    let memory = match MemorySubstrate::new(db_path) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            eprintln!("  [X] Failed to initialize memory: {}", e);
            return 1;
        }
    };

    // Create the LLM driver.
    let driver = match create_driver(&config.default_model) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  [X] Failed to create LLM driver: {}", e);
            return 1;
        }
    };

    // Create the Ring.
    let ring = Arc::new(Ring::new(config.clone(), memory, driver));

    // Spawn MCP servers from configuration.
    ring.spawn_mcp_servers().await;

    // Auto-load bundled gorilla manifests.
    let gorilla_count = load_bundled_gorillas(&ring);

    // Register built-in workflows.
    let workflow_count = register_builtin_workflows(&ring);

    // Auto-spawn a default fighter if none are running.
    if ring.list_fighters().is_empty() {
        let manifest = punch_types::FighterManifest {
            name: "Punch".to_string(),
            description: "The default all-rounder fighter.".to_string(),
            model: config.default_model.clone(),
            system_prompt:
                "You are Punch, a personal AI assistant with real capabilities. You have tools \
                 that let you read calendars, send emails, search the web, read files, and more. \
                 When the user asks you to do something, USE your tools — don't say you can't. \
                 If a tool fails, explain what happened and suggest alternatives. \
                 Be helpful, concise, and direct. Take action, don't just talk about it."
                    .to_string(),
            capabilities: Capability::full_access(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        };
        let id = ring.spawn_fighter(manifest).await;
        info!(%id, "auto-spawned default fighter: Punch");
    }

    // Write PID file.
    let pid_path = punch_home().join(".daemon.pid");
    let pid = std::process::id();
    if let Err(e) = std::fs::write(&pid_path, pid.to_string()) {
        eprintln!("  [!] Failed to write PID file: {}", e);
    }

    // Print startup banner.
    println!("{}", STARTUP_BANNER);
    println!("  Listening on: http://{}", config.api_listen);
    println!("  Provider:     {}", config.default_model.provider);
    println!("  Model:        {}", config.default_model.model);
    println!("  Gorillas:     {} registered", gorilla_count);
    println!("  Workflows:    {} registered", workflow_count);
    println!("  MCP Servers:  {} active", ring.mcp_clients().len());
    let fighter_count = ring.list_fighters().len();
    println!("  Fighters:     {} ready", fighter_count);
    println!("  Channels:     {} configured", config.channels.len());
    if !config.channels.is_empty() {
        for (name, ch) in &config.channels {
            println!("    - {} ({})", name, ch.channel_type);
        }
    }
    println!("  PID:          {}", pid);
    println!();

    info!(
        address = %config.api_listen,
        provider = %config.default_model.provider,
        gorillas = gorilla_count,
        pid = pid,
        "punch daemon starting"
    );

    // Set up Ctrl+C handler for graceful shutdown.
    let ring_shutdown = Arc::clone(&ring);
    let pid_path_clone = pid_path.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            println!();
            println!("  Shutting down gracefully...");
            ring_shutdown.shutdown();
            // Remove PID file.
            let _ = std::fs::remove_file(&pid_path_clone);
            println!("  The ring is empty. Goodbye.");
            std::process::exit(0);
        }
    });

    // Start the Arena (HTTP server). This blocks until shutdown.
    if let Err(e) = start_arena(ring, &config).await {
        eprintln!("  [X] Arena error: {}", e);
        // Clean up PID file on error.
        let _ = std::fs::remove_file(&pid_path);
        return 1;
    }

    // Clean up PID file on normal exit.
    let _ = std::fs::remove_file(&pid_path);

    0
}

/// Load gorilla manifests from the bundled directory and register them with the Ring.
fn load_bundled_gorillas(ring: &Arc<Ring>) -> usize {
    let mut count = 0;

    // Look for GORILLA.toml files in the bundled gorillas directory.
    // Try multiple locations: cargo manifest dir, or relative to binary.
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

            match std::fs::read_to_string(&toml_path) {
                Ok(contents) => match toml::from_str::<GorillaManifest>(&contents) {
                    Ok(manifest) => {
                        let name = manifest.name.clone();
                        ring.register_gorilla(manifest);
                        info!(name = %name, path = %toml_path.display(), "loaded bundled gorilla");
                        count += 1;
                    }
                    Err(e) => {
                        eprintln!("  [!] Failed to parse {}: {}", toml_path.display(), e);
                    }
                },
                Err(e) => {
                    eprintln!("  [!] Failed to read {}: {}", toml_path.display(), e);
                }
            }
        }
    }

    count
}

/// Register built-in workflows with the Ring.
fn register_builtin_workflows(ring: &Arc<Ring>) -> usize {
    // research-and-summarize: Step 1 = Scout (researches topic), Step 2 = Oracle (summarizes findings)
    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "research-and-summarize".to_string(),
        steps: vec![
            WorkflowStep {
                name: "scout".to_string(),
                fighter_name: "Scout".to_string(),
                prompt_template: "Research the following topic thoroughly and provide detailed findings:\n\n{{input}}".to_string(),
                timeout_secs: Some(120),
                on_error: OnError::FailWorkflow,
            },
            WorkflowStep {
                name: "oracle".to_string(),
                fighter_name: "Oracle".to_string(),
                prompt_template: "Summarize the following research findings into a clear, concise summary:\n\n{{previous_output}}".to_string(),
                timeout_secs: Some(120),
                on_error: OnError::FailWorkflow,
            },
        ],
    };

    ring.register_workflow(workflow);
    info!("registered built-in workflow: research-and-summarize");

    1
}
