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

fn default_config(provider: &str, model: &str, api_key_env: &str) -> String {
    let key_line = if api_key_env.is_empty() {
        "# api_key_env = \"\"  # Ollama needs no key".to_string()
    } else {
        format!("api_key_env = \"{}\"", api_key_env)
    };

    format!(
        r#"# Punch Configuration
# https://punch.sh/docs/config

# Arena API server address
api_listen = "127.0.0.1:6660"

# Default model configuration
[default_model]
provider = "{}"
model = "{}"
{}
max_tokens = 8192
temperature = 0.7

# Memory subsystem
[memory]
db_path = "~/.punch/data/memory.db"
knowledge_graph_enabled = true
"#,
        provider, model, key_line
    )
}

/// Default model for each provider.
fn default_model_for_provider(provider: &str) -> &str {
    match provider {
        "anthropic" => "claude-sonnet-4-20250514",
        "openai" => "gpt-4o",
        "google" => "gemini-2.0-flash",
        "groq" => "llama-3.3-70b-versatile",
        "ollama" => "llama3.2",
        "deepseek" => "deepseek-chat",
        _ => "claude-sonnet-4-20250514",
    }
}

/// Try to auto-detect available Ollama models.
/// Runs the blocking HTTP call in a separate thread to avoid async runtime issues.
fn detect_ollama_models() -> Vec<String> {
    let handle = std::thread::spawn(|| {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
        {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        match client.get("http://localhost:11434/api/tags").send() {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.json::<serde_json::Value>()
                    && let Some(models) = body["models"].as_array()
                {
                    return models
                        .iter()
                        .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
                        .collect();
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    });
    handle.join().unwrap_or_default()
}

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
        home.join("agents"),
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

    // Model selection.
    let model = if provider == "ollama" {
        select_ollama_model()
    } else {
        default_model_for_provider(provider).to_string()
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

    // Write config.toml.
    let config_content = default_config(provider, &model, api_key_env);
    let config_path = home.join("config.toml");
    if let Err(e) = std::fs::write(&config_path, &config_content) {
        eprintln!("  [X] Failed to write config: {}", e);
        return 1;
    }
    println!("  [+] Wrote config to {}", config_path.display());
    println!("  [+] Provider: {}, Model: {}", provider, model);

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

/// When Ollama is selected, auto-detect models and let user pick.
fn select_ollama_model() -> String {
    println!();
    println!("  Detecting Ollama models...");

    let models = detect_ollama_models();

    if models.is_empty() {
        println!("  [!] Ollama not reachable or no models installed.");
        println!("      Make sure Ollama is running: ollama serve");
        println!("      Install a model: ollama pull llama3.2");
        println!();
        println!("  Using default model: llama3.2");
        return "llama3.2".to_string();
    }

    println!("  Found {} model(s):", models.len());
    println!();

    for (i, model) in models.iter().enumerate() {
        let marker = if i == 0 { " (default)" } else { "" };
        println!("    {}) {}{}", i + 1, model, marker);
    }

    print!("\n  > Choose [1]: ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();

    if input.is_empty() {
        return models[0].clone();
    }

    if let Ok(idx) = input.parse::<usize>()
        && idx >= 1
        && idx <= models.len()
    {
        return models[idx - 1].clone();
    }

    // If they typed a model name directly, use it.
    if !input.is_empty() {
        return input.to_string();
    }

    models[0].clone()
}
