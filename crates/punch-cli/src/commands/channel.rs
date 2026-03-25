//! `punch channel` — Manage channel adapters.

use crate::cli::ChannelCommands;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// Base URL for the Punch daemon API.
fn api_base() -> String {
    std::env::var("PUNCH_API_URL").unwrap_or_else(|_| "http://127.0.0.1:6660".to_string())
}

pub async fn run(command: ChannelCommands, config_path: Option<String>) -> i32 {
    match command {
        ChannelCommands::List => run_list(config_path).await,
        ChannelCommands::Setup { platform } => run_setup(platform).await,
        ChannelCommands::Tunnel { action } => run_tunnel(action, config_path),
        ChannelCommands::Remove { platform } => run_remove(&platform),
        ChannelCommands::Test { platform } => run_test(&platform, config_path).await,
        ChannelCommands::Status { name } => run_status(&name).await,
    }
}

// ---------------------------------------------------------------------------
// Setup wizard
// ---------------------------------------------------------------------------

/// Prompt the user for input and return the trimmed response.
fn prompt(text: &str) -> String {
    print!("  > {}: ", text);
    std::io::stdout().flush().ok();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf).ok();
    buf.trim().to_string()
}

/// Prompt with a default value shown in brackets.
fn prompt_default(text: &str, default: &str) -> String {
    print!("  > {} [{}]: ", text, default);
    std::io::stdout().flush().ok();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf).ok();
    let val = buf.trim().to_string();
    if val.is_empty() {
        default.to_string()
    } else {
        val
    }
}

/// Generate a 32-byte hex secret using `rand`.
fn generate_secret() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen()).collect();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Check if `cloudflared` is installed and return its version string.
fn detect_cloudflared() -> Option<String> {
    std::process::Command::new("cloudflared")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let out = String::from_utf8_lossy(&o.stderr).to_string();
                // cloudflared prints version to stderr
                let version = out.trim().to_string();
                if version.is_empty() {
                    let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    Some(out)
                } else {
                    Some(version)
                }
            } else {
                None
            }
        })
}

/// Spawn `cloudflared tunnel --url` and wait for the public URL.
/// Returns `(url, child_process)`.
fn start_tunnel(port: u16) -> Result<(String, std::process::Child), String> {
    use std::process::{Command, Stdio};

    let mut child = Command::new("cloudflared")
        .args(["tunnel", "--url", &format!("http://127.0.0.1:{}", port)])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start cloudflared: {}", e))?;

    let stderr = child
        .stderr
        .take()
        .ok_or("Failed to capture cloudflared stderr")?;
    let reader = BufReader::new(stderr);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    for line in reader.lines() {
        if std::time::Instant::now() > deadline {
            break;
        }
        if let Ok(line) = line
            && let Some(url) = parse_tunnel_url(&line)
        {
            return Ok((url, child));
        }
    }

    // Kill the process on timeout
    child.kill().ok();
    Err("Timed out waiting for tunnel URL (15s). You can enter a URL manually.".to_string())
}

/// Extract a `https://*.trycloudflare.com` URL from a log line.
fn parse_tunnel_url(line: &str) -> Option<String> {
    // cloudflared logs contain the URL like: https://something-random.trycloudflare.com
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.contains(".trycloudflare.com") {
        Some(url.to_string())
    } else {
        None
    }
}

/// Register a Telegram webhook via the Bot API.
async fn register_telegram_webhook(token: &str, url: &str, secret: &str) -> Result<(), String> {
    let api_url = format!("https://api.telegram.org/bot{}/setWebhook", token);
    let client = reqwest::Client::new();
    let resp = client
        .post(&api_url)
        .json(&serde_json::json!({
            "url": url,
            "secret_token": secret,
            "drop_pending_updates": true,
        }))
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    if body["ok"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err(format!(
            "Telegram API error: {}",
            body["description"].as_str().unwrap_or("unknown error")
        ))
    }
}

/// Append a TOML block to the config file (preserves existing content/comments).
fn append_channel_config(path: &Path, toml_block: &str) -> Result<(), String> {
    use std::fs::OpenOptions;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("Failed to open config: {}", e))?;

    // Add a blank line separator before the new block
    write!(file, "\n{}", toml_block).map_err(|e| format!("Failed to write config: {}", e))?;

    Ok(())
}

/// Upsert key=value pairs in a .env file. Creates the file if it doesn't exist.
fn update_env_file(path: &Path, vars: &[(String, String)]) -> Result<(), String> {
    let existing = if path.exists() {
        std::fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };

    let mut lines: Vec<String> = existing.lines().map(|l| l.to_string()).collect();

    for (key, value) in vars {
        let prefix = format!("{}=", key);
        let new_line = format!("{}={}", key, value);
        if let Some(pos) = lines.iter().position(|l| l.starts_with(&prefix)) {
            lines[pos] = new_line;
        } else {
            lines.push(new_line);
        }
    }

    // Ensure trailing newline
    let content = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    std::fs::write(path, content).map_err(|e| format!("Failed to write .env file: {}", e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tunnel management
// ---------------------------------------------------------------------------

fn run_tunnel(action: Option<crate::cli::TunnelAction>, config_path: Option<String>) -> i32 {
    let punch_home = super::punch_home();
    let config_file = punch_home.join("config.toml");

    match action {
        // `punch channel tunnel` (no subcommand) — show current tunnel
        None => match load_saved_tunnel(config_path.as_deref()) {
            Some((url, mode)) => {
                println!();
                println!("  Tunnel configuration:");
                println!("    URL:    {}", url);
                println!("    Mode:   {}", mode);
                println!("    Config: {}", config_file.display());
                println!();
                println!("  All channel webhooks use this base URL:");
                println!("    <url>/api/channels/telegram/webhook");
                println!("    <url>/api/channels/slack/events");
                println!("    <url>/api/channels/discord/webhook");
                println!();
                println!("  To change:  punch channel tunnel set <new-url>");
                println!("  To remove:  punch channel tunnel remove");
                println!();
                0
            }
            None => {
                println!();
                println!("  No tunnel configured.");
                println!();
                println!("  Run `punch channel setup <platform>` to set one up,");
                println!("  or set it directly:");
                println!();
                println!("    punch channel tunnel set https://your-url.com");
                println!("    punch channel tunnel set https://your-url.com --mode named");
                println!();
                0
            }
        },
        // `punch channel tunnel set <url>`
        Some(crate::cli::TunnelAction::Set { url, mode }) => {
            let url = url.trim_end_matches('/');
            if !url.starts_with("https://") && !url.starts_with("http://") {
                eprintln!("  [X] URL must start with https:// or http://");
                return 1;
            }

            let config_content = std::fs::read_to_string(&config_file).unwrap_or_default();

            if config_content.contains("[tunnel]") {
                // Replace existing tunnel block
                match replace_toml_section(
                    &config_content,
                    "tunnel",
                    &format!("[tunnel]\nbase_url = \"{}\"\nmode = \"{}\"\n", url, mode),
                ) {
                    Ok(new_content) => {
                        if let Err(e) = std::fs::write(&config_file, new_content) {
                            eprintln!("  [X] Failed to write config: {}", e);
                            return 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("  [X] {}", e);
                        return 1;
                    }
                }
            } else {
                // Append new tunnel block
                let block = format!("\n[tunnel]\nbase_url = \"{}\"\nmode = \"{}\"\n", url, mode);
                if let Err(e) = append_channel_config(&config_file, &block) {
                    eprintln!("  [X] {}", e);
                    return 1;
                }
            }

            println!();
            println!("  [+] Tunnel URL updated: {}", url);
            println!("  [+] Mode: {}", mode);
            println!();
            println!("  NOTE: Existing webhook registrations still point to the old URL.");
            println!("  You may need to re-register webhooks with each platform:");
            println!("    punch channel setup telegram   (re-registers automatically)");
            println!();
            0
        }
        // `punch channel tunnel remove`
        Some(crate::cli::TunnelAction::Remove) => {
            let config_content = std::fs::read_to_string(&config_file).unwrap_or_default();

            if !config_content.contains("[tunnel]") {
                println!("  No tunnel configured. Nothing to remove.");
                return 0;
            }

            match replace_toml_section(&config_content, "tunnel", "") {
                Ok(new_content) => {
                    if let Err(e) = std::fs::write(&config_file, new_content) {
                        eprintln!("  [X] Failed to write config: {}", e);
                        return 1;
                    }
                }
                Err(e) => {
                    eprintln!("  [X] {}", e);
                    return 1;
                }
            }

            println!();
            println!(
                "  [+] Tunnel configuration removed from {}",
                config_file.display()
            );
            println!();
            println!("  Webhook registrations with platforms are still active.");
            println!("  They'll start failing once the tunnel is stopped.");
            println!();
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Channel removal
// ---------------------------------------------------------------------------

fn run_remove(platform: &str) -> i32 {
    let platform = platform.to_lowercase();
    let punch_home = super::punch_home();
    let config_file = punch_home.join("config.toml");
    let env_file = punch_home.join(".env");

    let config_content = std::fs::read_to_string(&config_file).unwrap_or_default();
    let section_name = format!("channels.{}", platform);

    if !config_content.contains(&format!("[{}]", section_name)) {
        eprintln!("  [X] No channel config found for '{}'.", platform);
        eprintln!("  Run `punch channel list` to see configured channels.");
        return 1;
    }

    // Remove the [channels.<platform>] section from config
    match replace_toml_section(&config_content, &section_name, "") {
        Ok(new_content) => {
            if let Err(e) = std::fs::write(&config_file, new_content) {
                eprintln!("  [X] Failed to write config: {}", e);
                return 1;
            }
        }
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    }

    // Remove related env vars from .env
    let platform_upper = platform.to_uppercase();
    let env_prefixes: Vec<String> = vec![
        format!("{}_BOT_TOKEN", platform_upper),
        format!("{}_WEBHOOK_SECRET", platform_upper),
        format!("{}_ACCESS_TOKEN", platform_upper),
        format!("{}_TOKEN", platform_upper),
        format!("{}_PHONE_NUMBER_ID", platform_upper),
    ];

    if env_file.exists()
        && let Ok(content) = std::fs::read_to_string(&env_file)
    {
        let filtered: Vec<&str> = content
            .lines()
            .filter(|line| !env_prefixes.iter().any(|prefix| line.starts_with(prefix)))
            .collect();
        let new_content = if filtered.is_empty() {
            String::new()
        } else {
            format!("{}\n", filtered.join("\n"))
        };
        if let Err(e) = std::fs::write(&env_file, new_content) {
            eprintln!("  [!] Config removed but failed to clean .env: {}", e);
        }
    }

    println!();
    println!("  [+] Removed '{}' channel.", platform);
    println!();
    println!("  Cleaned up:");
    println!(
        "    Config: {} (removed [{}])",
        config_file.display(),
        section_name
    );
    println!(
        "    Secrets: {} (removed {} vars)",
        env_file.display(),
        platform_upper
    );
    println!();
    println!(
        "  NOTE: The webhook registration with {} is still active.",
        platform
    );
    println!("  It will keep sending requests until you deregister it on the platform side.");
    println!();
    0
}

/// Replace or remove a TOML section in a config string.
/// Finds `[section_name]` and replaces everything up to the next `[` header or EOF.
fn replace_toml_section(
    content: &str,
    section_name: &str,
    replacement: &str,
) -> Result<String, String> {
    let header = format!("[{}]", section_name);
    let start = content
        .find(&header)
        .ok_or_else(|| format!("Section [{}] not found", section_name))?;

    // Find the end — next section header or EOF
    let after_header = start + header.len();
    let end = content[after_header..]
        .find("\n[")
        .map(|pos| after_header + pos + 1) // +1 to keep the newline before next section
        .unwrap_or(content.len());

    let mut result = String::new();
    result.push_str(&content[..start]);
    if !replacement.is_empty() {
        result.push_str(replacement);
    }
    result.push_str(&content[end..]);

    // Clean up multiple blank lines
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }

    Ok(result)
}

/// Try to load an existing tunnel base_url from config.
fn load_saved_tunnel(config_path: Option<&str>) -> Option<(String, String)> {
    let config = super::load_config(config_path).ok()?;
    let tunnel = config.tunnel?;
    Some((tunnel.base_url, tunnel.mode))
}

/// Resolve the public base URL — reuse saved config or run through tunnel setup.
/// Returns `(base_url, tunnel_mode)` where mode is "quick", "named", or "manual".
fn resolve_base_url(saved: Option<(String, String)>) -> Option<(String, String)> {
    // Check if we already have a saved tunnel URL
    if let Some((ref url, ref mode)) = saved {
        println!("  Existing tunnel configured:");
        println!("    URL:  {}", url);
        println!("    Mode: {}", mode);
        println!();
        let reuse = prompt_default("Use this URL for webhooks?", "Y");
        if reuse.to_lowercase() == "y" || reuse.is_empty() {
            return saved;
        }
        println!();
    }

    println!("  How will you expose this machine to the internet?");
    println!();
    println!("    1. Local testing    — quick tunnel, temporary URL, restarts break it");
    println!("    2. Persistent access — named tunnel, stable URL, survives restarts");
    println!("    3. I have my own URL — paste any public URL you control");
    println!();
    let choice = prompt("Select [1/2/3]");

    match choice.as_str() {
        "1" => resolve_quick_tunnel(),
        "2" => resolve_named_tunnel(),
        _ => resolve_manual_url(),
    }
}

/// Option 1: Quick tunnel (temporary, dev/testing).
fn resolve_quick_tunnel() -> Option<(String, String)> {
    println!();
    match detect_cloudflared() {
        Some(version) => {
            println!(
                "  [+] cloudflared detected ({})",
                version.lines().next().unwrap_or(&version)
            );
            println!("  [+] Starting tunnel to 127.0.0.1:6660...");
            println!();
            println!("  WARNING: This URL changes every time you restart the tunnel.");
            println!("           You'll need to re-register webhooks after each restart.");
            println!("           For persistent access, use option 2 (named tunnel).");
            println!();
            match start_tunnel(6660) {
                Ok((url, _child)) => {
                    println!("  [+] Tunnel URL: {}", url);
                    Some((url, "quick".to_string()))
                }
                Err(e) => {
                    eprintln!("  [X] Failed to start tunnel: {}", e);
                    println!();
                    println!("  Falling back to manual URL entry.");
                    resolve_manual_url()
                }
            }
        }
        None => {
            println!("  cloudflared is not installed.");
            println!();
            println!("  Install it first:");
            println!();
            println!("    # macOS");
            println!("    brew install cloudflare/cloudflare/cloudflared");
            println!();
            println!("    # Linux (Debian/Ubuntu)");
            println!(
                "    curl -L https://pkg.cloudflare.com/cloudflared-stable-linux-amd64.deb -o cloudflared.deb"
            );
            println!("    sudo dpkg -i cloudflared.deb");
            println!();
            println!("    # Other platforms");
            println!(
                "    https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/"
            );
            println!();
            println!("  After installing, re-run: punch channel setup telegram");
            println!();
            let fallback = prompt_default(
                "Or enter a public URL manually instead? (leave empty to exit)",
                "",
            );
            if fallback.is_empty() {
                None
            } else {
                Some((
                    fallback.trim_end_matches('/').to_string(),
                    "manual".to_string(),
                ))
            }
        }
    }
}

/// Option 2: Named tunnel (persistent, production-ready).
fn resolve_named_tunnel() -> Option<(String, String)> {
    println!();
    println!("  Named tunnels give you a stable URL that survives restarts.");
    println!("  You need a free Cloudflare account and a domain on Cloudflare.");
    println!();

    if detect_cloudflared().is_none() {
        println!("  cloudflared is not installed. Install it first:");
        println!();
        println!("    # macOS");
        println!("    brew install cloudflare/cloudflare/cloudflared");
        println!();
        println!("    # Linux (Debian/Ubuntu)");
        println!(
            "    curl -L https://pkg.cloudflare.com/cloudflared-stable-linux-amd64.deb -o cloudflared.deb"
        );
        println!("    sudo dpkg -i cloudflared.deb");
        println!();
        println!("    # Other platforms");
        println!(
            "    https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/"
        );
        println!();
        println!("  After installing, run these commands to set up a named tunnel:");
    } else {
        println!("  If you haven't set one up yet, run these commands:");
    }

    println!();
    println!("    cloudflared tunnel login");
    println!("    cloudflared tunnel create punch");
    println!("    cloudflared tunnel route dns punch channels.yourdomain.com");
    println!();
    println!("  Then start it with:");
    println!("    cloudflared tunnel run punch");
    println!();
    let url = prompt("Enter your named tunnel URL (e.g., https://channels.yourdomain.com)");
    if url.is_empty() {
        None
    } else {
        Some((url.trim_end_matches('/').to_string(), "named".to_string()))
    }
}

/// Option 3: Manual URL (BYO reverse proxy, ngrok, etc).
fn resolve_manual_url() -> Option<(String, String)> {
    println!();
    let url = prompt("Enter your public URL (e.g., https://yourdomain.com)");
    if url.is_empty() {
        None
    } else {
        Some((url.trim_end_matches('/').to_string(), "manual".to_string()))
    }
}

async fn run_setup(platform: Option<String>) -> i32 {
    let platforms = punch_channels::onboarding::available_platforms();

    let platform = match platform {
        Some(p) => p.to_lowercase(),
        None => {
            println!();
            println!("  Available platforms:");
            println!();
            for (i, (id, display)) in platforms.iter().enumerate() {
                println!("    {}. {} ({})", i + 1, display, id);
            }
            println!();
            let choice = prompt("Select a platform (number or name)");
            if let Ok(n) = choice.parse::<usize>() {
                if n >= 1 && n <= platforms.len() {
                    platforms[n - 1].0.to_string()
                } else {
                    eprintln!("  [X] Invalid selection.");
                    return 1;
                }
            } else {
                choice.to_lowercase()
            }
        }
    };

    let guide = match punch_channels::onboarding::guide_for(&platform) {
        Some(g) => g,
        None => {
            eprintln!(
                "  [X] Unknown platform: {}. Available: {}",
                platform,
                platforms
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return 1;
        }
    };

    println!();
    println!("  === Channel Deployment: {} ===", guide.display_name);
    println!();

    // Show platform-specific setup steps
    for (i, step) in guide.steps.iter().enumerate() {
        println!("  Step {}: {}", i + 1, step.instruction);
        if let Some(url) = step.url {
            println!("          {}", url);
        }
    }
    println!();

    print!("  Press Enter when ready...");
    std::io::stdout().flush().ok();
    let mut _buf = String::new();
    std::io::stdin().read_line(&mut _buf).ok();

    // Collect credentials
    let mut env_vars: Vec<(String, String)> = Vec::new();

    for cred in &guide.credentials {
        let value = prompt(cred.prompt);
        if value.is_empty() {
            eprintln!("  [X] {} is required.", cred.prompt);
            return 1;
        }
        if cred.is_secret && value.len() > 8 {
            println!("  [+] Token saved ({}...)", &value[..8]);
        } else {
            println!("  [+] Saved: {}", value);
        }
        env_vars.push((cred.env_var.to_string(), value));
    }

    // Platform-specific: collect allowed user IDs
    let mut allowed_user_ids: Vec<String> = Vec::new();
    if platform == "telegram" {
        println!();
        println!("  To find your numeric user ID, message @userinfobot on Telegram.");
        println!("  It will reply with your ID (e.g., 8514018060).");
        println!("  Do NOT use your @username — Telegram webhooks use numeric IDs.");
        loop {
            let user_id = prompt("Enter your Telegram numeric user ID");
            if user_id.is_empty() {
                println!("  [!] Skipping allowlist — anyone can message this bot.");
                break;
            }
            if user_id.starts_with('@') {
                println!("  [!] That looks like a username, not a numeric ID.");
                println!("      Message @userinfobot on Telegram to get your numeric ID.");
                continue;
            }
            if user_id.parse::<i64>().is_err() {
                println!("  [!] User ID must be numeric (e.g., 8514018060).");
                continue;
            }
            println!("  [+] Allowlist: {}", user_id);
            allowed_user_ids.push(user_id);

            let more = prompt_default("Add another user ID? [y/N]", "N");
            if !more.eq_ignore_ascii_case("y") {
                break;
            }
        }
    }

    // Collect or generate webhook secret (platform-dependent).
    // Telegram accepts any secret_token we set, so we generate one.
    // Slack and Discord have their own signing secrets in the platform dashboard.
    let webhook_secret = match platform.as_str() {
        "slack" => {
            println!();
            println!("  Slack uses its own signing secret for webhook verification.");
            println!(
                "  Find it at: https://api.slack.com/apps → your app → Basic Information → Signing Secret"
            );
            let secret = prompt("Paste your Slack signing secret");
            if secret.is_empty() {
                eprintln!("  [X] Slack signing secret is required for webhook verification.");
                return 1;
            }
            println!(
                "  [+] Slack signing secret saved ({}...)",
                &secret[..secret.len().min(8)]
            );
            secret
        }
        "discord" => {
            println!();
            println!("  Discord uses its own public key for webhook verification.");
            println!(
                "  Find it at: https://discord.com/developers/applications → your app → General Information → Public Key"
            );
            let key = prompt("Paste your Discord public key");
            if key.is_empty() {
                eprintln!("  [X] Discord public key is required for webhook verification.");
                return 1;
            }
            println!(
                "  [+] Discord public key saved ({}...)",
                &key[..key.len().min(8)]
            );
            key
        }
        _ => {
            // Telegram and other platforms: generate a random secret
            let secret = generate_secret();
            println!();
            println!("  [+] Generated webhook secret: {}...", &secret[..16]);
            secret
        }
    };
    let secret_env_var = format!("{}_WEBHOOK_SECRET", platform.to_uppercase());
    env_vars.push((secret_env_var.clone(), webhook_secret.clone()));

    // --- Tunnel / public URL (shared across all channels) ---
    println!();
    let saved_tunnel = load_saved_tunnel(None);
    let (base_url, tunnel_mode) = match resolve_base_url(saved_tunnel) {
        Some(pair) => pair,
        None => {
            eprintln!("  [X] A public URL is required for webhook registration.");
            return 1;
        }
    };

    let webhook_url = format!("{}{}", base_url, guide.webhook_path);

    // Register webhook (platform-specific)
    if platform == "telegram" {
        let token = env_vars
            .iter()
            .find(|(k, _)| k == "TELEGRAM_BOT_TOKEN")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");

        println!();
        println!("  [+] Registering webhook with Telegram...");
        match register_telegram_webhook(token, &webhook_url, &webhook_secret).await {
            Ok(()) => println!("  [+] Webhook registered!"),
            Err(e) => {
                eprintln!("  [X] Failed to register webhook: {}", e);
                eprintln!("      You can register manually later:");
                eprintln!("      curl -X POST 'https://api.telegram.org/bot<TOKEN>/setWebhook' \\",);
                eprintln!("        -d 'url={}'", webhook_url);
            }
        }
    }

    // --- Write config ---
    let punch_home = super::punch_home();
    let config_file = punch_home.join("config.toml");
    let env_file = punch_home.join(".env");

    // Save tunnel config (shared — only written once, subsequent channels reuse it)
    let tunnel_block = format!(
        r#"[tunnel]
base_url = "{base_url}"
mode = "{mode}"
"#,
        base_url = base_url,
        mode = tunnel_mode,
    );

    // Write tunnel config: append if new, update if URL changed.
    let existing_config = std::fs::read_to_string(&config_file).unwrap_or_default();
    if !existing_config.contains("[tunnel]") {
        // New tunnel — append it
        println!();
        println!("  Writing tunnel config to {}...", config_file.display());
        if let Err(e) = append_channel_config(&config_file, &tunnel_block) {
            eprintln!("  [X] {}", e);
            return 1;
        }
    } else if load_saved_tunnel(None).map(|(url, _)| url) != Some(base_url.clone()) {
        // Tunnel URL changed — update the existing [tunnel] section
        println!();
        println!("  Updating tunnel config in {}...", config_file.display());
        match replace_toml_section(&existing_config, "tunnel", &tunnel_block) {
            Ok(new_content) => {
                if let Err(e) = std::fs::write(&config_file, new_content) {
                    eprintln!("  [X] Failed to write config: {}", e);
                    return 1;
                }
            }
            Err(e) => {
                eprintln!("  [X] {}", e);
                return 1;
            }
        }
    }

    // Save channel config
    let allowed_ids_toml = if allowed_user_ids.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            allowed_user_ids
                .iter()
                .map(|id| format!("\"{}\"", id))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let token_env_key = guide
        .credentials
        .first()
        .map(|c| c.env_var)
        .unwrap_or("TOKEN");

    let channel_block = format!(
        r#"[channels.{platform}]
channel_type = "{platform}"
token_env = "{token_env}"
webhook_secret_env = "{secret_env}"
allowed_user_ids = {allowed_ids}
rate_limit_per_user = 20
"#,
        platform = platform,
        token_env = token_env_key,
        secret_env = secret_env_var,
        allowed_ids = allowed_ids_toml,
    );

    println!("  Writing channel config to {}...", config_file.display());
    let existing_config = std::fs::read_to_string(&config_file).unwrap_or_default();
    let section_name = format!("channels.{}", platform);
    if existing_config.contains(&format!("[{}]", section_name)) {
        // Update existing channel config instead of appending a duplicate
        match replace_toml_section(&existing_config, &section_name, &channel_block) {
            Ok(new_content) => {
                if let Err(e) = std::fs::write(&config_file, new_content) {
                    eprintln!("  [X] Failed to write config: {}", e);
                    return 1;
                }
            }
            Err(e) => {
                eprintln!("  [X] {}", e);
                return 1;
            }
        }
    } else {
        // Append new channel config
        if let Err(e) = append_channel_config(&config_file, &channel_block) {
            eprintln!("  [X] {}", e);
            return 1;
        }
    }

    println!("  Writing secrets to {}...", env_file.display());
    if let Err(e) = update_env_file(&env_file, &env_vars) {
        eprintln!("  [X] {}", e);
        return 1;
    }

    // Success banner
    println!();
    println!("  ========================================");
    println!("    {} channel is battle-ready!", guide.display_name);
    println!("  ========================================");
    println!();
    println!("  Webhook:  {}", webhook_url);
    println!("  Tunnel:   {} ({})", base_url, tunnel_mode);
    println!("  Security: signature verification + allowlist + 20 msg/min rate limit");
    println!();
    if tunnel_mode == "quick" {
        println!("  NOTE: Quick tunnel URL changes on restart. Re-run this wizard or");
        println!("        manually update the webhook if you restart the tunnel.");
        println!();
    }
    println!("  Next steps:");
    println!("    1. punch start");
    println!("    2. punch fighter spawn scout");
    println!("    3. Message your bot on {}!", guide.display_name);
    println!();
    println!("  Manage your setup:");
    println!("    punch channel list                  — see all channels");
    println!("    punch channel setup slack            — add another channel (same tunnel)");
    println!("    punch channel tunnel                 — show tunnel URL");
    println!("    punch channel tunnel set <url>       — change tunnel URL");
    println!("    punch channel tunnel remove          — remove tunnel config");
    println!(
        "    punch channel remove {}           — remove this channel + secrets",
        platform
    );
    println!();
    println!("  Files:");
    println!("    Config:  {}", config_file.display());
    println!("    Secrets: {}", env_file.display());
    println!();

    0
}

async fn run_list(config_path: Option<String>) -> i32 {
    let config = match super::load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    if config.channels.is_empty() {
        println!("  No channels configured.");
        println!();
        println!("  Add channels to ~/.punch/config.toml:");
        println!();
        println!("  [channels.telegram]");
        println!("  channel_type = \"telegram\"");
        println!("  token_env = \"TELEGRAM_BOT_TOKEN\"");
        println!();
        println!("  [channels.discord]");
        println!("  channel_type = \"discord\"");
        println!("  token_env = \"DISCORD_BOT_TOKEN\"");
        println!();
        println!("  [channels.slack]");
        println!("  channel_type = \"slack\"");
        println!("  token_env = \"SLACK_BOT_TOKEN\"");
        return 0;
    }

    println!("  Configured channels:");
    println!();

    for (name, channel_config) in &config.channels {
        let token_status = if let Some(ref env_var) = channel_config.token_env {
            if std::env::var(env_var).is_ok() {
                "set"
            } else {
                "NOT SET"
            }
        } else {
            "no token configured"
        };

        let default_fighter = channel_config
            .settings
            .get("default_fighter")
            .and_then(|v| v.as_str())
            .unwrap_or("(none)");

        println!("  {} ({})", name, channel_config.channel_type);
        println!("    Token:           {}", token_status);
        println!("    Default Fighter: {}", default_fighter);

        let webhook_path = match channel_config.channel_type.as_str() {
            "telegram" => "/api/channels/telegram/webhook",
            "discord" => "/api/channels/discord/webhook",
            "slack" => "/api/channels/slack/events",
            _ => "(unknown)",
        };
        println!("    Webhook:         POST {}", webhook_path);
        println!();
    }

    0
}

async fn run_test(platform: &str, config_path: Option<String>) -> i32 {
    let config = match super::load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] {}", e);
            return 1;
        }
    };

    let base_url = format!("http://{}", config.api_listen);
    let client = reqwest::Client::new();

    let (endpoint, test_payload) = match platform {
        "telegram" => (
            format!("{}/api/channels/telegram/webhook", base_url),
            serde_json::json!({
                "update_id": 999999,
                "message": {
                    "message_id": 1,
                    "from": { "id": 12345, "first_name": "Test", "last_name": "User" },
                    "chat": { "id": 12345, "type": "private" },
                    "date": chrono::Utc::now().timestamp(),
                    "text": "Hello from punch channel test!"
                }
            }),
        ),
        "discord" => (
            format!("{}/api/channels/discord/webhook", base_url),
            serde_json::json!({
                "id": "test_msg_1",
                "channel_id": "test_channel",
                "content": "Hello from punch channel test!",
                "author": {
                    "id": "test_user_1",
                    "username": "test_user",
                    "bot": false
                },
                "timestamp": chrono::Utc::now().to_rfc3339()
            }),
        ),
        "slack" => (
            format!("{}/api/channels/slack/events", base_url),
            serde_json::json!({
                "type": "event_callback",
                "event": {
                    "type": "message",
                    "user": "U_TEST_USER",
                    "channel": "C_TEST_CHANNEL",
                    "text": "Hello from punch channel test!",
                    "ts": format!("{}.000100", chrono::Utc::now().timestamp())
                }
            }),
        ),
        _ => {
            eprintln!(
                "  [X] Unknown platform: {}. Use: telegram, discord, slack",
                platform
            );
            return 1;
        }
    };

    println!("  Testing {} channel...", platform);
    println!("  Endpoint: POST {}", endpoint);
    println!();

    match client.post(&endpoint).json(&test_payload).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_default();

            if status.is_success() {
                println!("  Status: {} OK", status.as_u16());
                if let Some(response) = body["response"].as_str() {
                    println!("  Response: {}", response);
                }
                if let Some(error) = body["error"].as_str() {
                    println!("  Note: {}", error);
                }
            } else {
                println!("  Status: {} FAILED", status.as_u16());
                println!(
                    "  Body: {}",
                    serde_json::to_string_pretty(&body).unwrap_or_default()
                );
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to connect: {}", e);
            eprintln!("  Make sure the daemon is running (`punch start`).");
            return 1;
        }
    }

    0
}

async fn run_status(name: &str) -> i32 {
    let url = format!("{}/api/channels/{}", api_base(), name);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] Failed to create HTTP client: {}", e);
            return 1;
        }
    };

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(data) => {
                println!();
                println!("  Channel: {}", data["name"].as_str().unwrap_or(name));
                println!(
                    "  Type:    {}",
                    data["channel_type"].as_str().unwrap_or("-")
                );
                println!(
                    "  Status:  {}",
                    data["status"].as_str().unwrap_or("unknown")
                );

                if let Some(connected) = data["connected"].as_bool() {
                    println!("  Connected: {}", if connected { "yes" } else { "no" });
                }

                if let Some(last_msg) = data["last_message_at"].as_str() {
                    println!("  Last message: {}", last_msg);
                }

                if let Some(msg_count) = data["message_count"].as_u64() {
                    println!("  Messages: {}", msg_count);
                }

                if let Some(fighter) = data["default_fighter"].as_str() {
                    println!("  Default Fighter: {}", fighter);
                }

                println!();
                0
            }
            Err(e) => {
                eprintln!("  [X] Failed to parse response: {}", e);
                1
            }
        },
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
            eprintln!("  [X] Channel '{}' not found.", name);
            eprintln!("  Run `punch channel list` to see configured channels.");
            1
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("  [X] API error ({}): {}", status, body);
            1
        }
        Err(e) => {
            if e.is_connect() {
                eprintln!("  [X] Cannot connect to Punch daemon at {}", api_base());
                eprintln!("      Is the daemon running? Try: punch start");
            } else {
                eprintln!("  [X] Request failed: {}", e);
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    /// Build the channels list URL.
    fn build_list_url(base: &str) -> String {
        format!("{}/api/channels", base)
    }

    /// Build the channel test URL.
    fn build_test_url(base: &str, platform: &str) -> String {
        match platform {
            "telegram" => format!("{}/api/channels/telegram/webhook", base),
            "discord" => format!("{}/api/channels/discord/webhook", base),
            "slack" => format!("{}/api/channels/slack/events", base),
            _ => format!("{}/api/channels/{}/test", base, platform),
        }
    }

    /// Build the channel status URL.
    fn build_status_url(base: &str, name: &str) -> String {
        format!("{}/api/channels/{}", base, name)
    }

    /// Format channel status for display.
    fn format_channel_status(data: &serde_json::Value) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "  Channel: {}",
            data["name"].as_str().unwrap_or("-")
        ));
        lines.push(format!(
            "  Type:    {}",
            data["channel_type"].as_str().unwrap_or("-")
        ));
        lines.push(format!(
            "  Status:  {}",
            data["status"].as_str().unwrap_or("unknown")
        ));
        lines.join("\n")
    }

    #[test]
    fn test_build_list_url() {
        assert_eq!(
            build_list_url("http://localhost:6660"),
            "http://localhost:6660/api/channels"
        );
    }

    #[test]
    fn test_build_test_url_telegram() {
        assert_eq!(
            build_test_url("http://localhost:6660", "telegram"),
            "http://localhost:6660/api/channels/telegram/webhook"
        );
    }

    #[test]
    fn test_build_test_url_discord() {
        assert_eq!(
            build_test_url("http://localhost:6660", "discord"),
            "http://localhost:6660/api/channels/discord/webhook"
        );
    }

    #[test]
    fn test_build_test_url_slack() {
        assert_eq!(
            build_test_url("http://localhost:6660", "slack"),
            "http://localhost:6660/api/channels/slack/events"
        );
    }

    #[test]
    fn test_build_status_url() {
        assert_eq!(
            build_status_url("http://localhost:6660", "telegram"),
            "http://localhost:6660/api/channels/telegram"
        );
    }

    #[test]
    fn test_format_channel_status() {
        let data = serde_json::json!({
            "name": "telegram",
            "channel_type": "telegram",
            "status": "connected",
        });
        let output = format_channel_status(&data);
        assert!(output.contains("telegram"));
        assert!(output.contains("connected"));
    }

    #[test]
    fn test_format_channel_status_missing_fields() {
        let data = serde_json::json!({});
        let output = format_channel_status(&data);
        assert!(output.contains("-"));
        assert!(output.contains("unknown"));
    }

    #[test]
    fn test_channel_status_response_parsing() {
        let data = serde_json::json!({
            "name": "discord",
            "channel_type": "discord",
            "status": "active",
            "connected": true,
            "message_count": 42,
            "default_fighter": "oracle",
        });
        assert_eq!(data["name"].as_str().unwrap(), "discord");
        assert!(data["connected"].as_bool().unwrap());
        assert_eq!(data["message_count"].as_u64().unwrap(), 42);
        assert_eq!(data["default_fighter"].as_str().unwrap(), "oracle");
    }

    #[test]
    fn test_api_base_default() {
        let url = build_status_url("http://127.0.0.1:6660", "slack");
        assert_eq!(url, "http://127.0.0.1:6660/api/channels/slack");
    }

    #[test]
    fn test_parse_tunnel_url_valid() {
        let line =
            "2024-01-01 INF |  https://random-words.trycloudflare.com  connection registered";
        assert_eq!(
            super::parse_tunnel_url(line),
            Some("https://random-words.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn test_parse_tunnel_url_no_match() {
        assert_eq!(super::parse_tunnel_url("some random log line"), None);
    }

    #[test]
    fn test_parse_tunnel_url_other_https() {
        let line = "connected to https://api.cloudflare.com/v4";
        assert_eq!(super::parse_tunnel_url(line), None);
    }

    #[test]
    fn test_parse_tunnel_url_at_end_of_line() {
        let line = "INF https://abc-def-ghi.trycloudflare.com";
        assert_eq!(
            super::parse_tunnel_url(line),
            Some("https://abc-def-ghi.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn test_generate_secret_length() {
        let secret = super::generate_secret();
        // 32 bytes = 64 hex chars
        assert_eq!(secret.len(), 64);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_secret_uniqueness() {
        let a = super::generate_secret();
        let b = super::generate_secret();
        assert_ne!(a, b);
    }

    #[test]
    fn test_update_env_file_create() {
        let dir = std::env::temp_dir().join("punch_test_env_create");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");

        super::update_env_file(
            &path,
            &[
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ],
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("FOO=bar"));
        assert!(content.contains("BAZ=qux"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_env_file_upsert() {
        let dir = std::env::temp_dir().join("punch_test_env_upsert");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");

        std::fs::write(&path, "FOO=old\nKEEP=yes\n").unwrap();

        super::update_env_file(&path, &[("FOO".to_string(), "new".to_string())]).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("FOO=new"));
        assert!(content.contains("KEEP=yes"));
        assert!(!content.contains("FOO=old"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_append_channel_config() {
        let dir = std::env::temp_dir().join("punch_test_config_append");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        std::fs::write(
            &path,
            "# Existing config\napi_listen = \"127.0.0.1:6660\"\n",
        )
        .unwrap();

        let block = "[channels.telegram]\nchannel_type = \"telegram\"\n";
        super::append_channel_config(&path, block).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Existing config"));
        assert!(content.contains("[channels.telegram]"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_replace_toml_section_middle() {
        let content = "[server]\nport = 8080\n\n[tunnel]\nbase_url = \"https://old.com\"\nmode = \"quick\"\n\n[channels.telegram]\nchannel_type = \"telegram\"\n";
        let result = super::replace_toml_section(
            content,
            "tunnel",
            "[tunnel]\nbase_url = \"https://new.com\"\nmode = \"named\"\n",
        )
        .unwrap();
        assert!(result.contains("https://new.com"));
        assert!(!result.contains("https://old.com"));
        assert!(result.contains("[server]"));
        assert!(result.contains("[channels.telegram]"));
    }

    #[test]
    fn test_replace_toml_section_remove() {
        let content = "[server]\nport = 8080\n\n[tunnel]\nbase_url = \"https://old.com\"\nmode = \"quick\"\n\n[channels.telegram]\nchannel_type = \"telegram\"\n";
        let result = super::replace_toml_section(content, "tunnel", "").unwrap();
        assert!(!result.contains("[tunnel]"));
        assert!(!result.contains("https://old.com"));
        assert!(result.contains("[server]"));
        assert!(result.contains("[channels.telegram]"));
    }

    #[test]
    fn test_replace_toml_section_at_end() {
        let content =
            "[server]\nport = 8080\n\n[tunnel]\nbase_url = \"https://old.com\"\nmode = \"quick\"\n";
        let result = super::replace_toml_section(content, "tunnel", "").unwrap();
        assert!(!result.contains("[tunnel]"));
        assert!(result.contains("[server]"));
    }

    #[test]
    fn test_replace_toml_section_not_found() {
        let content = "[server]\nport = 8080\n";
        let result = super::replace_toml_section(content, "tunnel", "new stuff");
        assert!(result.is_err());
    }

    #[test]
    fn test_replace_toml_section_channel_remove() {
        let content = "[tunnel]\nbase_url = \"https://x.com\"\n\n[channels.telegram]\nchannel_type = \"telegram\"\ntoken_env = \"TG_TOKEN\"\n\n[channels.slack]\nchannel_type = \"slack\"\n";
        let result = super::replace_toml_section(content, "channels.telegram", "").unwrap();
        assert!(!result.contains("[channels.telegram]"));
        assert!(!result.contains("TG_TOKEN"));
        assert!(result.contains("[channels.slack]"));
        assert!(result.contains("[tunnel]"));
    }
}
