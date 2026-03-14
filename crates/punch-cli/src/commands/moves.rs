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
        MoveCommands::Install { name } => run_install(name).await,
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
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
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
                        println!(
                            "  {:<24}  {:<12}  DESCRIPTION",
                            "NAME", "TYPE"
                        );
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
            }
        }
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
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(moves) => {
                    if let Some(arr) = moves.as_array() {
                        if arr.is_empty() {
                            println!("  No moves found matching \"{}\".", query);
                            println!();
                            return 0;
                        }

                        println!(
                            "  {:<24}  {:<12}  DESCRIPTION",
                            "NAME", "TYPE"
                        );
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
            }
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
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(data) => {
                    println!();
                    println!(
                        "  Move: {}",
                        data["name"].as_str().unwrap_or(&name)
                    );
                    println!(
                        "  Type: {}",
                        data["type"].as_str().unwrap_or("built-in")
                    );
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
            }
        }
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

async fn run_install(name: String) -> i32 {
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
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
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
            }
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
            eprintln!("  [X] Move '{}' not found in registry.", name);
            eprintln!("  To install manually, place a .toml definition in:");
            eprintln!(
                "    {}",
                super::punch_home().join("moves").display()
            );
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
        lines.push(format!(
            "  {:<24}  {:<12}  DESCRIPTION",
            "NAME", "TYPE"
        ));
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
        lines.push(format!(
            "  Move: {}",
            data["name"].as_str().unwrap_or("-")
        ));
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
