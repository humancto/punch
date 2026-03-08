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

    // Try to check if daemon is running by looking for a PID file.
    let pid_path = home.join("punch.pid");
    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            println!("  Daemon PID:   {}", pid_str.trim());
        }
    } else {
        println!("  Daemon:       not running");
    }

    println!();
    0
}

pub async fn run_stop() -> i32 {
    let home = punch_home();
    let pid_path = home.join("punch.pid");

    if !pid_path.exists() {
        println!();
        println!("  No running daemon found.");
        println!("  (No PID file at {})", pid_path.display());
        println!();
        return 1;
    }

    match std::fs::read_to_string(&pid_path) {
        Ok(pid_str) => {
            let pid_str = pid_str.trim();
            println!();
            println!("  Stopping Punch daemon (PID {})...", pid_str);

            // Attempt to send SIGTERM via the nix crate or a simple kill command.
            if let Ok(pid) = pid_str.parse::<i32>() {
                #[cfg(unix)]
                {
                    use std::process::Command;
                    let _ = Command::new("kill").arg(pid.to_string()).status();
                }

                // Remove PID file.
                let _ = std::fs::remove_file(&pid_path);
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
        // Try to parse it.
        match std::fs::read_to_string(&config_path) {
            Ok(contents) => match toml::from_str::<punch_types::PunchConfig>(&contents) {
                Ok(_) => println!("  [OK] Config file is valid"),
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

    // Check 5: Common API key env vars.
    let key_vars = [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GOOGLE_API_KEY",
        "GROQ_API_KEY",
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
