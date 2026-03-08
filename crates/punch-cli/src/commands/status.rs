//! `punch status`, `punch stop`, `punch doctor` — Daemon status commands.

use super::punch_home;

pub async fn run_status() -> i32 {
    let home = punch_home();

    println!();
    println!("  PUNCH STATUS");
    println!("  ============");
    println!();

    // Check if initialized.
    if !home.exists() {
        println!("  Initialized:  NO");
        println!();
        println!("  Run `punch init` to get started.");
        println!();
        return 1;
    }
    println!("  Initialized:  YES ({})", home.display());

    // Check config.
    let config_path = home.join("config.toml");
    if config_path.exists() {
        println!("  Config:       {}", config_path.display());
    } else {
        println!("  Config:       MISSING");
    }

    // Check .env.
    let env_path = home.join(".env");
    if env_path.exists() {
        println!("  Env file:     {}", env_path.display());
    } else {
        println!("  Env file:     not set (no API keys configured)");
    }

    // Check data directory.
    let data_path = home.join("data");
    if data_path.exists() {
        println!("  Data dir:     {}", data_path.display());
    } else {
        println!("  Data dir:     MISSING");
    }

    // Check memory database.
    let db_path = home.join("data").join("memory.db");
    if db_path.exists() {
        let meta = std::fs::metadata(&db_path);
        let size = meta.map(|m| m.len()).unwrap_or(0);
        println!("  Memory DB:    {} ({} bytes)", db_path.display(), size);
    } else {
        println!("  Memory DB:    not created yet");
    }

    // Check if daemon is running via PID file and health endpoint.
    let pid_path = home.join(".daemon.pid");
    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            let pid_str = pid_str.trim();
            // Check if process is still alive.
            let alive = is_process_alive(pid_str);
            if alive {
                println!("  Daemon PID:   {} (running)", pid_str);
                // Try health endpoint.
                if let Ok(config_contents) = std::fs::read_to_string(&config_path)
                    && let Ok(config) = toml::from_str::<punch_types::PunchConfig>(&config_contents)
                {
                    let health_url = format!("http://{}/health", config.api_listen);
                    match check_url_reachable(&health_url) {
                        true => println!("  Arena:        http://{} (healthy)", config.api_listen),
                        false => println!(
                            "  Arena:        http://{} (not responding)",
                            config.api_listen
                        ),
                    }
                }
            } else {
                println!(
                    "  Daemon:       stale PID file (process {} not running)",
                    pid_str
                );
            }
        }
    } else {
        println!("  Daemon:       not running");
    }

    println!();
    0
}

pub async fn run_stop() -> i32 {
    let home = punch_home();
    let pid_path = home.join(".daemon.pid");

    if !pid_path.exists() {
        // Also check legacy location.
        let legacy_pid = home.join("punch.pid");
        if !legacy_pid.exists() {
            println!();
            println!("  No running daemon found.");
            println!("  (No PID file at {})", pid_path.display());
            println!();
            return 1;
        }
    }

    // Try both PID file locations.
    let pid_file = if home.join(".daemon.pid").exists() {
        home.join(".daemon.pid")
    } else {
        home.join("punch.pid")
    };

    match std::fs::read_to_string(&pid_file) {
        Ok(pid_str) => {
            let pid_str = pid_str.trim();
            println!();
            println!("  Stopping Punch daemon (PID {})...", pid_str);

            if let Ok(pid) = pid_str.parse::<i32>() {
                #[cfg(unix)]
                {
                    use std::process::Command;
                    let _ = Command::new("kill").arg(pid.to_string()).status();
                }

                // Remove PID file.
                let _ = std::fs::remove_file(&pid_file);
                println!("  Daemon stopped. The ring is empty.");
                println!();
                0
            } else {
                eprintln!("  [X] Invalid PID in file: {}", pid_str);
                1
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to read PID file: {}", e);
            1
        }
    }
}

pub async fn run_doctor() -> i32 {
    println!();
    println!("  PUNCH DOCTOR");
    println!("  ============");
    println!("  Running health diagnostics...");
    println!();

    let mut issues = 0;

    let home = punch_home();

    // Check 1: Punch home directory.
    if home.exists() {
        println!("  [OK] Punch home directory exists ({})", home.display());
    } else {
        println!("  [!!] Punch home directory missing. Run `punch init`.");
        issues += 1;
    }

    // Check 2: Config file.
    let config_path = home.join("config.toml");
    if config_path.exists() {
        match std::fs::read_to_string(&config_path) {
            Ok(contents) => match toml::from_str::<punch_types::PunchConfig>(&contents) {
                Ok(config) => {
                    println!("  [OK] Config file is valid");
                    println!(
                        "       Provider: {}, Model: {}",
                        config.default_model.provider, config.default_model.model
                    );
                }
                Err(e) => {
                    println!("  [!!] Config file has errors: {}", e);
                    issues += 1;
                }
            },
            Err(e) => {
                println!("  [!!] Cannot read config file: {}", e);
                issues += 1;
            }
        }
    } else {
        println!("  [!!] Config file missing at {}", config_path.display());
        issues += 1;
    }

    // Check 3: .env / API key.
    let env_path = home.join(".env");
    if env_path.exists() {
        println!("  [OK] Env file present");
    } else {
        println!("  [--] No .env file (API keys may be set in environment)");
    }

    // Check 4: Data directory.
    let data_dir = home.join("data");
    if data_dir.exists() {
        println!("  [OK] Data directory exists");
    } else {
        println!("  [!!] Data directory missing");
        issues += 1;
    }

    // Check 5: SQLite DB exists and is readable.
    let db_path = home.join("data").join("memory.db");
    if db_path.exists() {
        match std::fs::metadata(&db_path) {
            Ok(meta) => {
                println!("  [OK] Memory database exists ({} bytes)", meta.len());
                // Try to open it.
                match punch_memory::MemorySubstrate::new(&db_path) {
                    Ok(_) => println!("  [OK] Memory database is readable"),
                    Err(e) => {
                        println!("  [!!] Memory database is corrupted: {}", e);
                        issues += 1;
                    }
                }
            }
            Err(e) => {
                println!("  [!!] Cannot stat memory database: {}", e);
                issues += 1;
            }
        }
    } else {
        println!("  [--] Memory database not created yet (will be created on first use)");
    }

    // Check 6: Ollama reachability.
    println!();
    println!("  LLM Provider Checks:");
    if check_url_reachable("http://localhost:11434/api/tags") {
        println!("  [OK] Ollama is reachable (localhost:11434)");
    } else {
        println!("  [--] Ollama is not reachable (localhost:11434)");
    }

    // Check 7: LM Studio reachability.
    if check_url_reachable("http://localhost:1234/v1/models") {
        println!("  [OK] LM Studio is reachable (localhost:1234)");
    } else {
        println!("  [--] LM Studio is not reachable (localhost:1234)");
    }

    // Check 8: Common API key env vars.
    println!();
    println!("  API Key Checks:");
    let key_vars = [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GOOGLE_API_KEY",
        "GROQ_API_KEY",
        "DEEPSEEK_API_KEY",
    ];
    let mut found_key = false;
    for var in &key_vars {
        if std::env::var(var).is_ok() {
            println!("  [OK] {} is set", var);
            found_key = true;
        }
    }
    if !found_key {
        println!("  [--] No common API key environment variables found");
        println!("       Set one or configure in ~/.punch/.env");
    }

    // Check 9: Daemon status.
    println!();
    println!("  Daemon Checks:");
    let pid_path = home.join(".daemon.pid");
    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            let pid_str = pid_str.trim();
            if is_process_alive(pid_str) {
                println!("  [OK] Daemon is running (PID {})", pid_str);

                // Check health endpoint.
                if let Ok(contents) = std::fs::read_to_string(&config_path)
                    && let Ok(config) = toml::from_str::<punch_types::PunchConfig>(&contents)
                {
                    let health_url = format!("http://{}/health", config.api_listen);
                    if check_url_reachable(&health_url) {
                        println!("  [OK] Arena health endpoint responding");
                    } else {
                        println!("  [!!] Arena health endpoint not responding");
                        issues += 1;
                    }
                }
            } else {
                println!(
                    "  [!!] Stale PID file (process {} not running). Remove with: punch stop",
                    pid_str
                );
                issues += 1;
            }
        }
    } else {
        println!("  [--] Daemon is not running");
    }

    // Summary.
    println!();
    if issues == 0 {
        println!("  All checks passed. Punch is fight-ready!");
    } else {
        println!(
            "  Found {} issue(s). Fix them and run `punch doctor` again.",
            issues
        );
    }
    println!();

    if issues > 0 { 1 } else { 0 }
}

/// Check if a URL is reachable with a short timeout.
/// Uses the async reqwest client to avoid blocking-in-async panics.
fn check_url_reachable(url: &str) -> bool {
    // We run this synchronously but we're inside a tokio runtime,
    // so use a blocking-safe approach via a thread.
    let url = url.to_string();
    let handle = std::thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        client
            .get(&url)
            .send()
            .is_ok_and(|r| r.status().is_success())
    });
    handle.join().unwrap_or(false)
}

/// Check if a process with the given PID string is alive.
fn is_process_alive(pid_str: &str) -> bool {
    #[cfg(unix)]
    {
        if let Ok(pid) = pid_str.parse::<i32>() {
            // kill(pid, 0) checks if process exists without sending a signal.
            use std::process::Command;
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .is_ok_and(|o| o.status.success())
        } else {
            false
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid_str;
        false
    }
}
