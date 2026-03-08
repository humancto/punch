//! `punch start` — Start the Punch daemon (Ring + Arena).

use std::sync::Arc;

use tracing::info;

use punch_api::server::start_arena;
use punch_kernel::Ring;
use punch_memory::MemorySubstrate;
use punch_runtime::create_driver;

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
    let db_path_str = config.memory.db_path.replace("~", &punch_home()
        .parent()
        .unwrap_or(&punch_home())
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default());
    let db_path_str = if config.memory.db_path.starts_with("~/.punch") {
        let home = dirs::home_dir().expect("could not determine home directory");
        config.memory.db_path.replace("~", &home.to_string_lossy())
    } else {
        db_path_str
    };
    let db_path = std::path::Path::new(&db_path_str);

    // Ensure the parent directory exists.
    if let Some(parent) = db_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("  [X] Failed to create data directory: {}", e);
            return 1;
        }
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

    // Print startup banner.
    println!("{}", STARTUP_BANNER);
    println!("  Listening on: http://{}", config.api_listen);
    println!("  Provider:     {}", config.default_model.provider);
    println!("  Model:        {}", config.default_model.model);
    println!();

    info!(
        address = %config.api_listen,
        provider = %config.default_model.provider,
        "punch daemon starting"
    );

    // Start the Arena (HTTP server). This blocks until shutdown.
    if let Err(e) = start_arena(ring, &config).await {
        eprintln!("  [X] Arena error: {}", e);
        return 1;
    }

    0
}
