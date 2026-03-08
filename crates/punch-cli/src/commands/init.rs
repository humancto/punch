//! `punch init` — Initialize the Punch environment.

use std::io::{self, Write};

use super::punch_home;

const BANNER: &str = r#"
    ____  __  ___   ________ __
   / __ \/ / / / | / / ____// /  / /
  / /_/ / / / /  |/ / /    / /__/ /
 / ____/ /_/ / /|  / /___ / __  /
/_/    \____/_/ |_/\____//_/ /_/

    The Agent Combat System
"#;

const DEFAULT_CONFIG: &str = r#"# Punch Configuration
# https://punch.sh/docs/config

# Arena API server address
api_listen = "127.0.0.1:6660"

# Default model configuration
[default_model]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
max_tokens = 8192
temperature = 0.7

# Memory subsystem
[memory]
db_path = "~/.punch/data/memory.db"
knowledge_graph_enabled = true
"#;

pub async fn run() -> i32 {
    println!("{}", BANNER);

    let home = punch_home();

    if home.exists() {
        println!("  [!] Punch is already initialized at {}", home.display());
        println!("      Delete the directory and re-run to start fresh.");
        return 1;
    }

    // Create directory structure.
    let dirs = [
        home.clone(),
        home.join("data"),
        home.join("fighters"),
        home.join("gorillas"),
        home.join("moves"),
    ];
    for dir in &dirs {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("  [X] Failed to create {}: {}", dir.display(), e);
            return 1;
        }
    }
    println!("  [+] Created {}", home.display());

    // Provider selection.
    println!();
    println!("  Select your LLM provider:");
    println!();
    let providers = [
        ("1", "anthropic", "Anthropic (Claude)"),
        ("2", "openai", "OpenAI (GPT)"),
        ("3", "google", "Google (Gemini)"),
        ("4", "groq", "Groq"),
        ("5", "ollama", "Ollama (local)"),
        ("6", "deepseek", "DeepSeek"),
    ];

    for (num, _, label) in &providers {
        println!("    {}) {}", num, label);
    }

    print!("\n  > Choose [1]: ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();

    let (provider, api_key_env) = match input {
        "2" => ("openai", "OPENAI_API_KEY"),
        "3" => ("google", "GOOGLE_API_KEY"),
        "4" => ("groq", "GROQ_API_KEY"),
        "5" => ("ollama", ""),
        "6" => ("deepseek", "DEEPSEEK_API_KEY"),
        _ => ("anthropic", "ANTHROPIC_API_KEY"),
    };

    // API key prompt (skip for Ollama).
    let mut api_key = String::new();
    if !api_key_env.is_empty() {
        println!();
        print!("  > Enter your {} (or press Enter to skip): ", api_key_env);
        io::stdout().flush().unwrap();
        io::stdin().read_line(&mut api_key).unwrap();
        api_key = api_key.trim().to_string();
    }

    // Write .env file.
    if !api_key.is_empty() {
        let env_path = home.join(".env");
        let env_content = format!("{}={}\n", api_key_env, api_key);
        if let Err(e) = std::fs::write(&env_path, env_content) {
            eprintln!("  [X] Failed to write .env: {}", e);
            return 1;
        }
        println!("  [+] Saved API key to {}", env_path.display());
    }

    // Write config.toml with selected provider.
    let config_content = DEFAULT_CONFIG.replace(
        "provider = \"anthropic\"",
        &format!("provider = \"{}\"", provider),
    );
    let config_content = if api_key_env.is_empty() {
        config_content.replace(
            "api_key_env = \"ANTHROPIC_API_KEY\"",
            "# api_key_env = \"\"  # Ollama needs no key",
        )
    } else {
        config_content.replace("ANTHROPIC_API_KEY", api_key_env)
    };

    let config_path = home.join("config.toml");
    if let Err(e) = std::fs::write(&config_path, config_content) {
        eprintln!("  [X] Failed to write config: {}", e);
        return 1;
    }
    println!("  [+] Wrote config to {}", config_path.display());

    // Success banner.
    println!();
    println!("  ========================================");
    println!("    Punch is ready to fight!");
    println!("  ========================================");
    println!();
    println!("  Next steps:");
    println!("    1. punch start           Start the daemon");
    println!("    2. punch fighter spawn   Spawn a fighter");
    println!("    3. punch chat            Start chatting");
    println!();
    println!("  Config: {}", config_path.display());
    println!("  Docs:   https://punch.sh/docs");
    println!();

    0
}
