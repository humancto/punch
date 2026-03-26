//! `punch fighter` — Manage fighters (conversational agents).

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use punch_kernel::Ring;
use punch_memory::MemorySubstrate;
use punch_runtime::create_driver;
use punch_types::config::ModelConfig;
use punch_types::{Capability, FighterId, FighterManifest, WeightClass};
use serde::Deserialize;
use uuid::Uuid;

use super::{load_config, load_dotenv, punch_home};
use crate::cli::FighterCommands;

// ---------------------------------------------------------------------------
// Agent template types
// ---------------------------------------------------------------------------

/// Intermediate TOML-friendly struct for loading agent.toml files.
/// Model is optional — it will be filled from global config if missing.
#[derive(Debug, Clone, Deserialize)]
struct AgentTemplate {
    name: String,
    description: String,
    #[serde(default = "default_weight_class")]
    weight_class: String,
    system_prompt: String,
    #[serde(default)]
    model: Option<AgentModelConfig>,
    #[serde(default)]
    capabilities: Vec<String>,
}

/// Optional model override in agent.toml.
#[derive(Debug, Clone, Deserialize)]
struct AgentModelConfig {
    provider: Option<String>,
    model: Option<String>,
    api_key_env: Option<String>,
    base_url: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
}

fn default_weight_class() -> String {
    "middleweight".to_string()
}

/// Resolve an AgentTemplate + global config into a FighterManifest.
fn resolve_template(template: AgentTemplate, default_model: &ModelConfig) -> FighterManifest {
    // Resolve model: start with global default, overlay template overrides.
    let model = match template.model {
        Some(agent_model) => {
            let provider = agent_model
                .provider
                .and_then(|p| serde_json::from_value(serde_json::Value::String(p)).ok())
                .unwrap_or_else(|| default_model.provider.clone());
            ModelConfig {
                provider,
                model: agent_model
                    .model
                    .unwrap_or_else(|| default_model.model.clone()),
                api_key_env: agent_model
                    .api_key_env
                    .or_else(|| default_model.api_key_env.clone()),
                base_url: agent_model
                    .base_url
                    .or_else(|| default_model.base_url.clone()),
                max_tokens: agent_model.max_tokens.or(default_model.max_tokens),
                temperature: agent_model.temperature.or(default_model.temperature),
            }
        }
        None => default_model.clone(),
    };

    // Parse weight class.
    let weight_class = match template.weight_class.to_lowercase().as_str() {
        "featherweight" => WeightClass::Featherweight,
        "heavyweight" => WeightClass::Heavyweight,
        "champion" => WeightClass::Champion,
        _ => WeightClass::Middleweight,
    };

    // Parse capabilities from string list.
    let capabilities = parse_capabilities(&template.capabilities);

    FighterManifest {
        name: template.name,
        description: template.description,
        model,
        system_prompt: template.system_prompt,
        capabilities,
        weight_class,
        tenant_id: None,
    }
}

/// Parse capability strings like "file_read", "shell_exec", "memory" into Capability enums.
fn parse_capabilities(caps: &[String]) -> Vec<Capability> {
    let mut result = Vec::new();
    for cap in caps {
        match cap.as_str() {
            "read_file" | "file_read" => result.push(Capability::FileRead("**".to_string())),
            "write_file" | "file_write" => result.push(Capability::FileWrite("**".to_string())),
            "shell_exec" => result.push(Capability::ShellExec("*".to_string())),
            "web_search" | "web_fetch" | "network" => {
                result.push(Capability::Network("*".to_string()))
            }
            "memory" | "memory_store" | "memory_recall" => result.push(Capability::Memory),
            "knowledge_graph" | "git_operations" => result.push(Capability::KnowledgeGraph),
            "browser_control" => result.push(Capability::BrowserControl),
            "agent_spawn" => result.push(Capability::AgentSpawn),
            "agent_message" => result.push(Capability::AgentMessage),
            "schedule" => result.push(Capability::Schedule),
            "event_publish" => result.push(Capability::EventPublish),
            "channel_notify" => result.push(Capability::ChannelNotify),
            "self_config" => result.push(Capability::SelfConfig),
            "system_automation" => result.push(Capability::SystemAutomation),
            s if s.starts_with("ui_automation(") && s.ends_with(')') => {
                let app = &s["ui_automation(".len()..s.len() - 1];
                result.push(Capability::UiAutomation(app.to_string()));
            }
            s if s.starts_with("app_integration(") && s.ends_with(')') => {
                let app = &s["app_integration(".len()..s.len() - 1];
                result.push(Capability::AppIntegration(app.to_string()));
            }
            _ => {
                // Unknown capability, skip silently.
            }
        }
    }
    // Deduplicate.
    result.dedup();
    result
}

// ---------------------------------------------------------------------------
// Template loading
// ---------------------------------------------------------------------------

/// Search paths for agent.toml files, in priority order:
/// 1. ./agents/{name}/agent.toml (project-local)
/// 2. ~/.punch/agents/{name}/agent.toml (user global)
fn find_agent_toml(template_name: &str) -> Option<PathBuf> {
    // 1. Project-local agents directory.
    let project_path = PathBuf::from("agents")
        .join(template_name)
        .join("agent.toml");
    if project_path.exists() {
        return Some(project_path);
    }

    // 2. User-global agents directory.
    let global_path = punch_home()
        .join("agents")
        .join(template_name)
        .join("agent.toml");
    if global_path.exists() {
        return Some(global_path);
    }

    None
}

/// Load a fighter template from disk or use a built-in default.
fn load_template(template: &str, default_model: &ModelConfig) -> Result<FighterManifest, String> {
    // 1. Try to load from agent.toml on disk.
    if let Some(path) = find_agent_toml(template) {
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let agent_template: AgentTemplate = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
        return Ok(resolve_template(agent_template, default_model));
    }

    // 2. Legacy path: ~/.punch/fighters/<template>.toml
    let legacy_path = punch_home()
        .join("fighters")
        .join(format!("{}.toml", template));
    if legacy_path.exists() {
        let contents = std::fs::read_to_string(&legacy_path)
            .map_err(|e| format!("Failed to read template: {}", e))?;
        // Try parsing as AgentTemplate first, then fall back to FighterManifest.
        if let Ok(agent_template) = toml::from_str::<AgentTemplate>(&contents) {
            return Ok(resolve_template(agent_template, default_model));
        }
        let manifest: FighterManifest =
            toml::from_str(&contents).map_err(|e| format!("Failed to parse template: {}", e))?;
        return Ok(manifest);
    }

    // 3. Built-in defaults.
    let manifest = match template {
        "default" | "punch" => FighterManifest {
            name: "Punch".to_string(),
            description: "The default all-rounder fighter.".to_string(),
            model: default_model.clone(),
            system_prompt:
                "You are Punch, a self-configuring AI assistant with real capabilities. You have \
                 tools that let you read calendars, send emails, search the web, read files, and \
                 more. You can also configure yourself: use heartbeat_add to set up recurring \
                 tasks (e.g., \"add a daily morning briefing\"), heartbeat_list/heartbeat_remove \
                 to manage them, skill_list to see available skill packs, skill_recommend to \
                 tell the user what to install and how (the user runs the install command), \
                 creed_view to inspect your own identity and configuration, \
                 and channel_notify to push messages to Telegram/Slack/Discord. \
                 When the user asks you to do something, USE your tools — don't say you can't. \
                 If a tool fails, explain what happened and suggest alternatives. \
                 Be helpful, concise, and direct. Take action, don't just talk about it."
                    .to_string(),
            capabilities: Capability::full_access(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        },
        "striker" => FighterManifest {
            name: "Striker".to_string(),
            description: "Expert full-stack software engineer.".to_string(),
            model: default_model.clone(),
            system_prompt: "You are Striker, an expert full-stack software engineer. You write \
                 production-quality code across all major languages and frameworks. \
                 Be thorough, handle errors, write tests."
                .to_string(),
            capabilities: Capability::full_access(),
            weight_class: WeightClass::Heavyweight,
            tenant_id: None,
        },
        "scout" => FighterManifest {
            name: "Scout".to_string(),
            description: "Deep research agent for thorough investigation.".to_string(),
            model: default_model.clone(),
            system_prompt: "You are Scout, a deep research agent. Investigate topics thoroughly, \
                 cross-reference sources, and produce well-cited reports. Be rigorous \
                 and evidence-based."
                .to_string(),
            capabilities: Capability::full_access(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        },
        "oracle" => FighterManifest {
            name: "Oracle".to_string(),
            description: "General-purpose conversational AI with broad knowledge.".to_string(),
            model: default_model.clone(),
            system_prompt: "You are Oracle, a knowledgeable and thoughtful AI assistant. You draw \
                 on broad knowledge to give insightful, well-reasoned answers. Be clear, \
                 concise, and helpful."
                .to_string(),
            capabilities: Capability::full_access(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        },
        "coder" => FighterManifest {
            name: "Coder".to_string(),
            description: "A heavyweight coding specialist.".to_string(),
            model: default_model.clone(),
            system_prompt: "You are Coder, a programming expert. Write clean, efficient code. \
                 Explain your reasoning. Always include error handling."
                .to_string(),
            capabilities: Capability::full_access(),
            weight_class: WeightClass::Heavyweight,
            tenant_id: None,
        },
        _ => {
            return Err(format!(
                "Unknown template '{}'. Available: default, striker, scout, oracle, coder\n  \
                 Or create an agent template at: agents/{}/agent.toml\n  \
                 Or place one at: {}",
                template,
                template,
                punch_home()
                    .join("agents")
                    .join(template)
                    .join("agent.toml")
                    .display()
            ));
        }
    };

    Ok(manifest)
}

// ---------------------------------------------------------------------------
// Daemon communication helpers
// ---------------------------------------------------------------------------

/// Try to read the daemon port from config. Returns the base URL if daemon appears reachable.
fn daemon_url(config_path: Option<&str>) -> Option<String> {
    let config = load_config(config_path).ok()?;
    let url = format!("http://{}", config.api_listen);
    let health_url = format!("{}/health", url);

    // Run blocking HTTP check in a separate thread to avoid async runtime panic.
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

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

pub async fn run(command: FighterCommands, config_path: Option<String>) -> i32 {
    load_dotenv();

    match command {
        FighterCommands::Spawn {
            template,
            name: _,
            model: _,
        } => run_spawn(template, config_path).await,
        FighterCommands::List => run_list(config_path).await,
        FighterCommands::Chat { name } => run_chat(name, config_path).await,
        FighterCommands::Send { id, message } => run_send(id, message, config_path).await,
        FighterCommands::Kill { id } => run_kill(id, config_path).await,
        FighterCommands::Status { id } => run_fighter_status(id, config_path).await,
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

    // Spawn oracle template as default for quick chat.
    let manifest = match load_template("oracle", &config.default_model) {
        Ok(m) => m,
        Err(_) => FighterManifest {
            name: "Punch".to_string(),
            description: "Default Punch fighter for quick chat.".to_string(),
            model: config.default_model.clone(),
            system_prompt: "You are Punch, a helpful AI assistant with the heart of a champion. \
                 Be concise, direct, and helpful."
                .to_string(),
            capabilities: vec![],
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        },
    };

    let fighter_id = ring.spawn_fighter(manifest).await;

    match message {
        Some(msg) => {
            // One-shot mode.
            match ring.send_message(&fighter_id, msg).await {
                Ok(result) => {
                    println!("{}", result.response);
                    if result.usage.total() > 0 {
                        eprintln!(
                            "  [tokens: {} in / {} out]",
                            result.usage.input_tokens, result.usage.output_tokens
                        );
                    }
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

    // Try to spawn against running daemon first.
    if let Some(base_url) = daemon_url(config_path.as_deref()) {
        let manifest = match load_template(&template, &config.default_model) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("  [X] {}", e);
                return 1;
            }
        };

        let client = reqwest::Client::new();
        let url = format!("{}/api/fighters", base_url);
        let body = serde_json::json!({ "manifest": manifest });

        match client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    let name = data["name"].as_str().unwrap_or(&template);
                    let id = data["id"].as_str().unwrap_or("unknown");
                    println!();
                    println!("  Fighter spawned (via daemon)!");
                    println!("  Name:  {}", name);
                    println!("  ID:    {}", id);
                    println!();
                    println!(
                        "  Start chatting: punch fighter chat {}",
                        name.to_lowercase()
                    );
                    println!();
                    return 0;
                }
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                eprintln!("  [X] Daemon returned {}: {}", status, body);
                return 1;
            }
            Err(e) => {
                eprintln!("  [X] Failed to reach daemon: {}", e);
                return 1;
            }
        }
    }

    // No daemon running — create in-process ring.
    let ring = match create_ring(&config).await {
        Ok(r) => r,
        Err(code) => return code,
    };

    let manifest = match load_template(&template, &config.default_model) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    let name = manifest.name.clone();
    let wc = manifest.weight_class;
    let id = ring.spawn_fighter(manifest).await;

    println!();
    println!("  Fighter spawned!");
    println!("  Name:  {}", name);
    println!("  ID:    {}", id);
    println!("  Class: {}", wc);
    println!();
    println!(
        "  Start chatting: punch fighter chat {}",
        name.to_lowercase()
    );
    println!();

    0
}

async fn run_list(config_path: Option<String>) -> i32 {
    // Try daemon first.
    if let Some(base_url) = daemon_url(config_path.as_deref()) {
        let client = reqwest::Client::new();
        let url = format!("{}/api/fighters", base_url);

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(fighters) = resp.json::<Vec<serde_json::Value>>().await {
                    if fighters.is_empty() {
                        println!();
                        println!("  No fighters in the ring.");
                        println!("  Spawn one: punch fighter spawn <template>");
                        println!();
                        return 0;
                    }

                    println!();
                    println!(
                        "  {:<36}  {:<16}  {:<14}  STATUS",
                        "ID", "NAME", "WEIGHT CLASS"
                    );
                    println!("  {}", "-".repeat(86));

                    for f in &fighters {
                        println!(
                            "  {:<36}  {:<16}  {:<14}  {}",
                            f["id"].as_str().unwrap_or("-"),
                            f["name"].as_str().unwrap_or("-"),
                            f["weight_class"].as_str().unwrap_or("-"),
                            f["status"].as_str().unwrap_or("-"),
                        );
                    }

                    println!();
                    println!("  Total: {} fighter(s) (via daemon)", fighters.len());
                    println!();
                    return 0;
                }
            }
            _ => {}
        }
    }

    // No daemon — check if we should inform the user.
    let config = match load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    // Check PID file to see if daemon *should* be running.
    let pid_path = punch_home().join(".daemon.pid");
    if pid_path.exists() {
        eprintln!("  [X] Daemon PID file exists but daemon is not responding.");
        eprintln!("      Try: punch stop && punch start");
        return 1;
    }

    println!();
    println!("  No daemon running. Showing in-process state.");
    println!();

    let ring = match create_ring(&config).await {
        Ok(r) => r,
        Err(code) => return code,
    };

    let fighters = ring.list_fighters();

    if fighters.is_empty() {
        println!("  No fighters in the ring.");
        println!("  Spawn one: punch fighter spawn <template>");
        println!();
        return 0;
    }

    println!(
        "  {:<36}  {:<16}  {:<14}  STATUS",
        "ID", "NAME", "WEIGHT CLASS"
    );
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

    // If a name is given, look up the fighter; otherwise spawn oracle.
    let fighter_id = match name {
        Some(ref n) => {
            // Try to load from template if the name matches a known template.
            let fighters = ring.list_fighters();
            match fighters
                .iter()
                .find(|(_, m, _)| m.name.eq_ignore_ascii_case(n))
            {
                Some((id, _, _)) => *id,
                None => {
                    // Try to spawn from template.
                    match load_template(n, &config.default_model) {
                        Ok(manifest) => ring.spawn_fighter(manifest).await,
                        Err(_) => {
                            eprintln!("  [X] Fighter '{}' not found and no matching template.", n);
                            return 1;
                        }
                    }
                }
            }
        }
        None => {
            let manifest = match load_template("oracle", &config.default_model) {
                Ok(m) => m,
                Err(_) => FighterManifest {
                    name: "Punch".to_string(),
                    description: "Default Punch fighter.".to_string(),
                    model: config.default_model.clone(),
                    system_prompt:
                        "You are Punch, a helpful AI assistant with the heart of a champion. \
                         Be concise, direct, and helpful."
                            .to_string(),
                    capabilities: vec![],
                    weight_class: WeightClass::Middleweight,
                    tenant_id: None,
                },
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
    println!("  Commands: /exit /quit /tools /status /memory");
    println!();

    print!("  you > ");
    io::stdout().flush().unwrap();

    let stdin = io::stdin();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let input = line.trim().to_string();

        if input.is_empty() {
            print!("  you > ");
            io::stdout().flush().unwrap();
            continue;
        }

        // Handle slash commands.
        if input == "/exit" || input == "/quit" || input == "/q" {
            println!("  Bell rings. Fight over.");
            break;
        }

        if input == "/tools" {
            let entry = ring.get_fighter(fighter_id);
            if let Some(entry) = entry {
                let tools = punch_runtime::tools_for_capabilities(&entry.manifest.capabilities);
                println!();
                println!("  Available tools ({}):", tools.len());
                for tool in &tools {
                    println!("    - {} : {}", tool.name, tool.description);
                }
                if tools.is_empty() {
                    println!("    (no tools — fighter has no capabilities)");
                }
                println!();
            }
            print!("  you > ");
            io::stdout().flush().unwrap();
            continue;
        }

        if input == "/status" {
            let entry = ring.get_fighter(fighter_id);
            if let Some(entry) = entry {
                println!();
                println!("  Fighter: {}", entry.manifest.name);
                println!("  Status:  {}", entry.status);
                println!("  Class:   {}", entry.manifest.weight_class);
                println!(
                    "  Model:   {} ({})",
                    entry.manifest.model.model, entry.manifest.model.provider
                );
                println!(
                    "  Bout:    {}",
                    entry
                        .current_bout
                        .map(|b| b.0.to_string())
                        .unwrap_or_else(|| "none".to_string())
                );
                println!();
            }
            print!("  you > ");
            io::stdout().flush().unwrap();
            continue;
        }

        if input == "/memory" {
            // Try to recall recent memories.
            println!();
            println!("  (Memory recall is available via the memory_recall tool during chat.)");
            println!();
            print!("  you > ");
            io::stdout().flush().unwrap();
            continue;
        }

        // Send message to fighter.
        match ring.send_message(fighter_id, input).await {
            Ok(result) => {
                println!();
                println!("  {} > {}", fighter_name, result.response);
                if result.usage.total() > 0 {
                    println!(
                        "  [tokens: {} in / {} out | tools: {} | iterations: {}]",
                        result.usage.input_tokens,
                        result.usage.output_tokens,
                        result.tool_calls_made,
                        result.iterations
                    );
                }
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

async fn run_fighter_status(id: String, config_path: Option<String>) -> i32 {
    // Try daemon first.
    if let Some(base_url) = daemon_url(config_path.as_deref()) {
        let client = reqwest::Client::new();
        let url = format!("{}/api/fighters/{}", base_url, id);

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    println!();
                    println!("  Fighter: {}", data["name"].as_str().unwrap_or("-"));
                    println!("  ID:      {}", data["id"].as_str().unwrap_or(&id));
                    println!("  Status:  {}", data["status"].as_str().unwrap_or("-"));
                    println!(
                        "  Class:   {}",
                        data["weight_class"].as_str().unwrap_or("-")
                    );
                    println!("  Model:   {}", data["model"].as_str().unwrap_or("-"));
                    if let Some(bout) = data["bout_id"].as_str() {
                        println!("  Bout:    {}", bout);
                    }
                    if let Some(msgs) = data["message_count"].as_u64() {
                        println!("  Messages: {}", msgs);
                    }
                    println!();
                    return 0;
                }
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                eprintln!("  [X] Fighter '{}' not found.", id);
                return 1;
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                eprintln!("  [X] API error ({}): {}", status, body);
                return 1;
            }
            Err(e) => {
                eprintln!("  [X] Failed to reach daemon: {}", e);
                return 1;
            }
        }
    }

    // No daemon — try in-process.
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

    match ring.get_fighter(&fighter_id) {
        Some(entry) => {
            println!();
            println!("  Fighter: {}", entry.manifest.name);
            println!("  ID:      {}", fighter_id);
            println!("  Status:  {}", entry.status);
            println!("  Class:   {}", entry.manifest.weight_class);
            println!(
                "  Model:   {} ({})",
                entry.manifest.model.model, entry.manifest.model.provider
            );
            println!(
                "  Bout:    {}",
                entry
                    .current_bout
                    .map(|b| b.0.to_string())
                    .unwrap_or_else(|| "none".to_string())
            );
            println!();
            0
        }
        None => {
            eprintln!("  [X] Fighter '{}' not found.", id);
            1
        }
    }
}

/// Create a Ring from config for CLI operations.
async fn create_ring(config: &punch_types::PunchConfig) -> Result<Arc<Ring>, i32> {
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
