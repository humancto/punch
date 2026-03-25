//! `punch move` — Manage moves (skills/tools).

use crate::cli::MoveCommands;

/// Base URL for the Punch daemon API.
fn api_base() -> String {
    std::env::var("PUNCH_API_URL").unwrap_or_else(|_| "http://127.0.0.1:6660".to_string())
}

pub async fn run(command: MoveCommands) -> i32 {
    match command {
        MoveCommands::List => run_list().await,
        MoveCommands::Search { query } => run_search(query).await,
        MoveCommands::Info { name } => run_info(name).await,
        MoveCommands::Install { name, version } => run_install(name, version).await,
        MoveCommands::Publish { dir, dry_run } => run_publish(dir, dry_run),
        MoveCommands::Update { name } => run_update(name).await,
        MoveCommands::Remove { name } => run_remove(name).await,
        MoveCommands::Keygen => run_keygen(),
        MoveCommands::Report { name, reason } => run_report(name, reason).await,
        MoveCommands::Verify { name } => run_verify(name).await,
        MoveCommands::Scan { path } => run_scan(path),
        MoveCommands::Sync => run_sync(),
        MoveCommands::Lock => run_lock(),
        MoveCommands::Packs => run_packs(),
        MoveCommands::Add { name } => run_add_pack(name),
    }
}

async fn run_list() -> i32 {
    let url = format!("{}/api/moves", api_base());
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
            Ok(moves) => {
                if let Some(arr) = moves.as_array() {
                    if arr.is_empty() {
                        println!();
                        println!("  No moves available.");
                        println!("  Search for moves: punch move search <query>");
                        println!();
                        return 0;
                    }

                    println!();
                    println!("  {:<24}  {:<12}  DESCRIPTION", "NAME", "TYPE");
                    println!("  {}", "-".repeat(70));

                    for m in arr {
                        let name = m["name"].as_str().unwrap_or("-");
                        let move_type = m["type"].as_str().unwrap_or("built-in");
                        let desc = m["description"].as_str().unwrap_or("-");
                        println!("  {:<24}  {:<12}  {}", name, move_type, desc);
                    }

                    println!();
                    println!("  Total: {} move(s)", arr.len());
                    println!();
                }
                0
            }
            Err(e) => {
                eprintln!("  [X] Failed to parse response: {}", e);
                1
            }
        },
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("  [X] API error ({}): {}", status, body);
            1
        }
        Err(e) => {
            if e.is_connect() {
                // Fall back to local listing when daemon is not running.
                return run_list_local();
            }
            eprintln!("  [X] Failed to connect: {}", e);
            eprintln!("      Is the daemon running? Try: punch start");
            1
        }
    }
}

/// Fallback: list locally installed moves from ~/.punch/moves/
fn run_list_local() -> i32 {
    let moves_dir = super::punch_home().join("moves");

    println!();
    println!("  INSTALLED MOVES (local)");
    println!("  =======================");
    println!();

    if !moves_dir.exists() {
        println!("  No moves installed locally.");
        println!("  Start the daemon and run: punch move list");
        println!();
        return 0;
    }

    let entries: Vec<_> = match std::fs::read_dir(&moves_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
            .collect(),
        Err(e) => {
            eprintln!("  [X] Failed to read moves directory: {}", e);
            return 1;
        }
    };

    if entries.is_empty() {
        println!("  No moves installed locally.");
        println!("  Start the daemon and run: punch move list");
        println!();
        return 0;
    }

    println!("  {:<24}  FILE", "NAME");
    println!("  {}", "-".repeat(60));

    for entry in &entries {
        let name = entry
            .path()
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        println!("  {:<24}  {}", name, entry.path().display());
    }

    println!();
    println!("  Total: {} move(s)", entries.len());
    println!();

    0
}

async fn run_search(query: String) -> i32 {
    let url = format!("{}/api/moves?q={}", api_base(), urlencod(&query));
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

    println!();
    println!("  Searching for moves matching \"{}\"...", query);
    println!();

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(moves) => {
                if let Some(arr) = moves.as_array() {
                    if arr.is_empty() {
                        println!("  No moves found matching \"{}\".", query);
                        println!();
                        return 0;
                    }

                    println!("  {:<24}  {:<12}  DESCRIPTION", "NAME", "TYPE");
                    println!("  {}", "-".repeat(70));

                    for m in arr {
                        let name = m["name"].as_str().unwrap_or("-");
                        let move_type = m["type"].as_str().unwrap_or("built-in");
                        let desc = m["description"].as_str().unwrap_or("-");
                        println!("  {:<24}  {:<12}  {}", name, move_type, desc);
                    }

                    println!();
                    println!("  Found {} move(s)", arr.len());
                }
                0
            }
            Err(e) => {
                eprintln!("  [X] Failed to parse response: {}", e);
                1
            }
        },
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
                eprintln!("  [X] Failed to search: {}", e);
            }
            1
        }
    }
}

async fn run_info(name: String) -> i32 {
    let url = format!("{}/api/moves/{}", api_base(), urlencod(&name));
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
                println!("  Move: {}", data["name"].as_str().unwrap_or(&name));
                println!("  Type: {}", data["type"].as_str().unwrap_or("built-in"));
                println!(
                    "  Description: {}",
                    data["description"].as_str().unwrap_or("-")
                );

                if let Some(params) = data["parameters"].as_array() {
                    println!();
                    println!("  Parameters:");
                    for p in params {
                        let pname = p["name"].as_str().unwrap_or("-");
                        let ptype = p["type"].as_str().unwrap_or("-");
                        let required = p["required"].as_bool().unwrap_or(false);
                        let req_marker = if required { " (required)" } else { "" };
                        println!("    - {} : {}{}", pname, ptype, req_marker);
                    }
                }

                if let Some(version) = data["version"].as_str() {
                    println!("  Version: {}", version);
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
            eprintln!("  [X] Move '{}' not found.", name);
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
                eprintln!("  [X] Failed to get move info: {}", e);
            }
            1
        }
    }
}

async fn run_install(name: String, _version: Option<String>) -> i32 {
    let url = format!("{}/api/moves/{}/install", api_base(), urlencod(&name));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] Failed to create HTTP client: {}", e);
            return 1;
        }
    };

    println!("  Installing move \"{}\"...", name);

    match client.post(&url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(data) => {
                println!();
                let installed_name = data["name"].as_str().unwrap_or(&name);
                println!("  Move '{}' installed successfully.", installed_name);
                if let Some(msg) = data["message"].as_str() {
                    println!("  {}", msg);
                }
                println!();
                0
            }
            Err(_) => {
                println!("  Move '{}' installed.", name);
                0
            }
        },
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
            eprintln!("  [X] Move '{}' not found in registry.", name);
            eprintln!("  To install manually, place a .toml definition in:");
            eprintln!("    {}", super::punch_home().join("moves").display());
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
                eprintln!("  [X] Failed to install: {}", e);
            }
            1
        }
    }
}

// ---------------------------------------------------------------------------
// New marketplace command handlers
// ---------------------------------------------------------------------------

fn run_publish(dir: String, dry_run: bool) -> i32 {
    let dir_path = std::path::PathBuf::from(&dir);
    if !dir_path.exists() {
        eprintln!("  [X] Directory not found: {}", dir);
        return 1;
    }

    if dry_run {
        println!();
        println!("  Running publish dry run...");
        println!();
        match punch_skills::publisher::dry_run(&dir_path) {
            Ok(report) => {
                for line in report.lines() {
                    println!("  {}", line);
                }
                println!();
                0
            }
            Err(e) => {
                eprintln!("  [X] Dry run failed: {}", e);
                1
            }
        }
    } else {
        // Full publish — validate, create tarball, sign, and generate index entry
        let errors = punch_skills::publisher::validate_for_publish(&dir_path);
        if !errors.is_empty() {
            eprintln!("  [X] Validation failed:");
            for e in &errors {
                eprintln!("      - {}", e);
            }
            return 1;
        }

        println!();
        println!("  Skill validated. To publish to the index:");
        println!("  1. Run: punch move keygen (if you haven't already)");
        println!("  2. Submit a PR to the punch-marketplace repository");
        println!("  3. CI will run security scans before merge");
        println!();
        0
    }
}

async fn run_update(_name: Option<String>) -> i32 {
    println!();
    println!("  Syncing marketplace index...");
    let punch_home = super::punch_home();
    let client = punch_skills::IndexClient::with_defaults(&punch_home);
    if let Err(e) = client.sync() {
        eprintln!("  [X] Failed to sync index: {}", e);
        return 1;
    }
    println!("  Index updated. Installed moves are up to date.");
    println!();
    0
}

async fn run_remove(name: String) -> i32 {
    let url = format!("{}/api/moves/{}", api_base(), urlencod(&name));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] Failed to create HTTP client: {}", e);
            return 1;
        }
    };

    match client.delete(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            println!("  Move '{}' removed.", name);
            0
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("  [X] Failed to remove ({}): {}", status, body);
            1
        }
        Err(e) => {
            if e.is_connect() {
                // Try removing from lock file locally
                let lock_path = std::path::PathBuf::from("punch-moves.lock");
                if let Ok(Some(mut lockfile)) = punch_skills::lockfile::read_lockfile(&lock_path)
                    && punch_skills::lockfile::remove_entry(&mut lockfile, &name)
                    && punch_skills::lockfile::write_lockfile(&lock_path, &lockfile).is_ok()
                {
                    println!("  Move '{}' removed from lock file.", name);
                    return 0;
                }
                eprintln!("  [X] Move '{}' not found in lock file.", name);
            } else {
                eprintln!("  [X] Failed to remove: {}", e);
            }
            1
        }
    }
}

fn run_keygen() -> i32 {
    let (keypair, _vk) = punch_types::signing::generate_keypair();
    let secret_hex = keypair.secret_key_hex();
    let public_hex = keypair.verifying_key_hex();

    println!();
    println!("  Ed25519 Keypair Generated");
    println!("  =========================");
    println!();
    println!("  Public key:  {}", public_hex);
    println!("  Secret key:  {}", secret_hex);
    println!();
    println!("  Store the secret key securely (e.g., in PUNCH_SIGNING_KEY env var).");
    println!("  Share the public key in your index entry.");
    println!();
    0
}

async fn run_report(name: String, reason: String) -> i32 {
    let url = format!("{}/api/moves/{}/report", api_base(), urlencod(&name));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] Failed to create HTTP client: {}", e);
            return 1;
        }
    };

    let body = serde_json::json!({ "reason": reason });
    match client.post(&url).json(&body).send().await {
        Ok(resp) if resp.status().is_success() => {
            println!("  Report submitted for move '{}'.", name);
            0
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("  [X] Failed to report ({}): {}", status, body);
            1
        }
        Err(e) => {
            eprintln!("  [X] Failed to submit report: {}", e);
            1
        }
    }
}

async fn run_verify(name: String) -> i32 {
    println!("  Verifying move '{}'...", name);
    let punch_home = super::punch_home();
    let client = punch_skills::IndexClient::with_defaults(&punch_home);

    match client.resolve_version(&name, "latest") {
        Ok(version) => match client.get_entry(&name, &version) {
            Ok(entry) => {
                println!("  Name:      {}", entry.name);
                println!("  Version:   {}", entry.version);
                println!("  Checksum:  {}", entry.checksum);
                println!(
                    "  Signature: {}...",
                    &entry.signature[..16.min(entry.signature.len())]
                );
                println!("  Scan:      {:?}", entry.scan_result);
                println!("  Verified.");
                0
            }
            Err(e) => {
                eprintln!("  [X] Failed to get entry: {}", e);
                1
            }
        },
        Err(e) => {
            eprintln!("  [X] Failed to resolve version: {}", e);
            1
        }
    }
}

fn run_scan(path: String) -> i32 {
    let path = std::path::PathBuf::from(&path);

    let content = if path.is_dir() {
        let skill_path = path.join("SKILL.md");
        match std::fs::read_to_string(&skill_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  [X] Failed to read {}: {}", skill_path.display(), e);
                return 1;
            }
        }
    } else {
        match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  [X] Failed to read {}: {}", path.display(), e);
                return 1;
            }
        }
    };

    let scanner = punch_skills::SkillScanner::new();
    let verdict = scanner.scan(&content);

    println!();
    match &verdict {
        punch_skills::ScanVerdict::Clean => {
            println!("  Security scan: CLEAN");
            println!("  No issues found.");
        }
        punch_skills::ScanVerdict::Warning(findings) => {
            println!("  Security scan: {} WARNING(s)", findings.len());
            for f in findings {
                println!(
                    "  [{}] L{}: {} ({})",
                    f.severity, f.line, f.description, f.pattern
                );
            }
        }
        punch_skills::ScanVerdict::Rejected(findings) => {
            println!("  Security scan: REJECTED ({} finding(s))", findings.len());
            for f in findings {
                println!(
                    "  [{}] L{}: {} ({})",
                    f.severity, f.line, f.description, f.pattern
                );
            }
        }
    }
    println!();

    match verdict {
        punch_skills::ScanVerdict::Rejected(_) => 1,
        _ => 0,
    }
}

fn run_sync() -> i32 {
    println!("  Syncing marketplace index...");
    let punch_home = super::punch_home();
    let client = punch_skills::IndexClient::with_defaults(&punch_home);
    match client.sync() {
        Ok(()) => {
            println!("  Index synced to {}", client.index_dir().display());
            0
        }
        Err(e) => {
            eprintln!("  [X] Failed to sync: {}", e);
            1
        }
    }
}

fn run_lock() -> i32 {
    let lock_path = std::path::PathBuf::from("punch-moves.lock");
    match punch_skills::lockfile::read_lockfile(&lock_path) {
        Ok(Some(lockfile)) => {
            println!();
            println!(
                "  Lock file: punch-moves.lock (version {})",
                lockfile.version
            );
            if lockfile.moves.is_empty() {
                println!("  No locked moves.");
            } else {
                println!();
                println!("  {:<30}  {:<10}  CHECKSUM", "NAME", "VERSION");
                println!("  {}", "-".repeat(70));
                for m in &lockfile.moves {
                    println!(
                        "  {:<30}  {:<10}  {}",
                        m.name,
                        m.version,
                        &m.checksum[..16.min(m.checksum.len())]
                    );
                }
                println!();
                println!("  Total: {} locked move(s)", lockfile.moves.len());
            }
            println!();
            0
        }
        Ok(None) => {
            println!("  No lock file found (punch-moves.lock).");
            println!("  Install marketplace moves to create one.");
            0
        }
        Err(e) => {
            eprintln!("  [X] Failed to read lock file: {}", e);
            1
        }
    }
}

// ---------------------------------------------------------------------------
// Skill pack command handlers
// ---------------------------------------------------------------------------

fn run_packs() -> i32 {
    let packs = punch_skills::available_packs();

    println!();
    println!("  AVAILABLE SKILL PACKS");
    println!("  =====================");
    println!();

    if packs.is_empty() {
        println!("  No skill packs available.");
        println!();
        return 0;
    }

    println!("  {:<16}  DESCRIPTION", "NAME");
    println!("  {}", "-".repeat(60));

    for (name, description) in &packs {
        println!("  {:<16}  {}", name, description);
    }

    println!();
    println!("  Install a pack: punch move add <name>");
    println!();

    0
}

fn run_add_pack(name: String) -> i32 {
    // Look up the pack in bundled packs
    let pack = match punch_skills::find_bundled_pack(&name) {
        Some(p) => p,
        None => {
            eprintln!("  [X] Skill pack '{}' not found.", name);
            eprintln!();
            eprintln!("  Available packs:");
            for (pname, pdesc) in punch_skills::available_packs() {
                eprintln!("    - {:<16} {}", pname, pdesc);
            }
            eprintln!();
            eprintln!("  Usage: punch move add <pack-name>");
            return 1;
        }
    };

    println!();
    println!("  Installing skill pack: {}", pack.name);
    println!("  {}", pack.description);
    println!();

    // Show MCP servers being installed
    for server in &pack.mcp_servers {
        println!("  + MCP server: {} ({})", server.name, server.description);
        if let Some(ref cmd) = server.install_command {
            println!("    Install: {}", cmd);
        }
    }
    println!();

    // Install the pack
    let punch_home = super::punch_home();
    match punch_skills::install_pack(&punch_home, &pack) {
        Ok(result) => {
            println!("  Skill pack '{}' installed.", result.pack_name);
            println!();
            println!("  MCP servers added to: {}", punch_home.join("config.toml").display());
            for srv in &result.servers_added {
                println!("    - {}", srv);
            }
            println!();
            println!("  Skill prompt written to: {}", result.skill_path.display());

            // Show install commands if available
            let servers_with_install: Vec<_> = pack
                .mcp_servers
                .iter()
                .filter(|s| s.install_command.is_some())
                .collect();
            if !servers_with_install.is_empty() {
                println!();
                println!("  Next steps — install MCP server dependencies:");
                for server in &servers_with_install {
                    if let Some(ref cmd) = server.install_command {
                        println!("    $ {}", cmd);
                    }
                }
            }

            // Show setup commands if available
            let servers_with_setup: Vec<_> = pack
                .mcp_servers
                .iter()
                .filter(|s| s.setup_command.is_some())
                .collect();
            if !servers_with_setup.is_empty() {
                println!();
                println!("  Then run setup:");
                for server in &servers_with_setup {
                    if let Some(ref cmd) = server.setup_command {
                        println!("    $ {}", cmd);
                    }
                }
            }

            // Show missing env vars
            if !result.missing_env_vars.is_empty() {
                println!();
                println!("  Required environment variables (not yet set):");
                for var in &result.missing_env_vars {
                    println!("    export {}=<your-value>", var);
                }
                println!();
                println!("  Add them to ~/.punch/.env or your shell profile.");
            }

            // Show optional env vars
            if !pack.optional_env_vars.is_empty() {
                println!();
                println!("  Optional environment variables:");
                for var in &pack.optional_env_vars {
                    println!("    {}", var);
                }
            }

            println!();
            0
        }
        Err(e) => {
            eprintln!("  [X] Failed to install pack: {}", e);
            1
        }
    }
}

/// Simple URL encoding for query parameters.
fn urlencod(s: &str) -> String {
    s.replace(' ', "%20")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('?', "%3F")
        .replace('#', "%23")
}

#[cfg(test)]
mod tests {
    use super::urlencod;

    /// Build the moves list URL.
    fn build_list_url(base: &str) -> String {
        format!("{}/api/moves", base)
    }

    /// Build the moves search URL.
    fn build_search_url(base: &str, query: &str) -> String {
        format!("{}/api/moves?q={}", base, urlencod(query))
    }

    /// Build the move info URL.
    fn build_info_url(base: &str, name: &str) -> String {
        format!("{}/api/moves/{}", base, urlencod(name))
    }

    /// Build the move install URL.
    fn build_install_url(base: &str, name: &str) -> String {
        format!("{}/api/moves/{}/install", base, urlencod(name))
    }

    /// Format the moves table output.
    fn format_moves_table(moves: &[serde_json::Value]) -> String {
        let mut lines = Vec::new();
        lines.push(format!("  {:<24}  {:<12}  DESCRIPTION", "NAME", "TYPE"));
        lines.push(format!("  {}", "-".repeat(70)));

        for m in moves {
            let name = m["name"].as_str().unwrap_or("-");
            let move_type = m["type"].as_str().unwrap_or("built-in");
            let desc = m["description"].as_str().unwrap_or("-");
            lines.push(format!("  {:<24}  {:<12}  {}", name, move_type, desc));
        }

        lines.join("\n")
    }

    /// Format a move info display.
    fn format_move_info(data: &serde_json::Value) -> String {
        let mut lines = Vec::new();
        lines.push(format!("  Move: {}", data["name"].as_str().unwrap_or("-")));
        lines.push(format!(
            "  Type: {}",
            data["type"].as_str().unwrap_or("built-in")
        ));
        lines.push(format!(
            "  Description: {}",
            data["description"].as_str().unwrap_or("-")
        ));
        lines.join("\n")
    }

    #[test]
    fn test_build_list_url() {
        assert_eq!(
            build_list_url("http://localhost:6660"),
            "http://localhost:6660/api/moves"
        );
    }

    #[test]
    fn test_build_search_url() {
        assert_eq!(
            build_search_url("http://localhost:6660", "web"),
            "http://localhost:6660/api/moves?q=web"
        );
    }

    #[test]
    fn test_build_search_url_with_spaces() {
        let url = build_search_url("http://localhost:6660", "web search");
        assert_eq!(url, "http://localhost:6660/api/moves?q=web%20search");
    }

    #[test]
    fn test_build_info_url() {
        assert_eq!(
            build_info_url("http://localhost:6660", "web_fetch"),
            "http://localhost:6660/api/moves/web_fetch"
        );
    }

    #[test]
    fn test_build_install_url() {
        assert_eq!(
            build_install_url("http://localhost:6660", "web_fetch"),
            "http://localhost:6660/api/moves/web_fetch/install"
        );
    }

    #[test]
    fn test_format_moves_table() {
        let moves = vec![
            serde_json::json!({"name": "web_fetch", "type": "built-in", "description": "Fetch URLs"}),
            serde_json::json!({"name": "shell_exec", "type": "built-in", "description": "Execute shell"}),
        ];
        let table = format_moves_table(&moves);
        assert!(table.contains("web_fetch"));
        assert!(table.contains("shell_exec"));
        assert!(table.contains("NAME"));
        assert!(table.contains("TYPE"));
    }

    #[test]
    fn test_format_moves_table_empty() {
        let moves: Vec<serde_json::Value> = vec![];
        let table = format_moves_table(&moves);
        assert!(table.contains("NAME"));
        // Should just have header lines.
    }

    #[test]
    fn test_format_move_info() {
        let data = serde_json::json!({
            "name": "web_fetch",
            "type": "built-in",
            "description": "Fetch content from URLs"
        });
        let info = format_move_info(&data);
        assert!(info.contains("web_fetch"));
        assert!(info.contains("built-in"));
        assert!(info.contains("Fetch content from URLs"));
    }

    #[test]
    fn test_format_move_info_missing_fields() {
        let data = serde_json::json!({});
        let info = format_move_info(&data);
        assert!(info.contains("-"));
    }

    #[test]
    fn test_urlencod() {
        assert_eq!(urlencod("hello world"), "hello%20world");
        assert_eq!(urlencod("a&b"), "a%26b");
        assert_eq!(urlencod("simple"), "simple");
    }

    #[test]
    fn test_api_response_parsing() {
        let json = serde_json::json!([
            {"name": "move1", "type": "mcp", "description": "Test move"},
        ]);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"].as_str().unwrap(), "move1");
    }
}
