//! `punch stats` — Show token usage and cost statistics.

use super::load_config;

pub async fn run(period: String, fighter: Option<String>) -> i32 {
    // Validate period.
    if !["hour", "day", "month"].contains(&period.as_str()) {
        eprintln!("  [X] Invalid period: {period}. Expected: hour, day, or month");
        return 1;
    }

    let base_url = match daemon_url(None) {
        Some(url) => url,
        None => {
            eprintln!("  [X] Daemon is not running. Start it with: punch start");
            return 1;
        }
    };

    let client = reqwest::Client::new();

    if let Some(ref fighter_ref) = fighter {
        // Per-fighter stats. First resolve the fighter ID.
        let fighter_id = match resolve_fighter_id(&client, &base_url, fighter_ref).await {
            Some(id) => id,
            None => {
                eprintln!("  [X] Fighter not found: {fighter_ref}");
                return 1;
            }
        };

        let url = format!(
            "{}/api/stats/fighters/{}?period={}",
            base_url, fighter_id, period
        );
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(stats) = resp.json::<serde_json::Value>().await {
                    print_fighter_stats(&stats);
                    return 0;
                }
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                eprintln!("  [X] API error ({status}): {body}");
            }
            Err(e) => {
                eprintln!("  [X] Request failed: {e}");
            }
        }
        return 1;
    }

    // Global stats.
    let url = format!("{}/api/stats?period={}", base_url, period);
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(stats) = resp.json::<serde_json::Value>().await {
                print_global_stats(&stats);
                return 0;
            }
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("  [X] API error ({status}): {body}");
        }
        Err(e) => {
            eprintln!("  [X] Request failed: {e}");
        }
    }
    1
}

/// Try to read the daemon port from config. Returns the base URL if daemon is reachable.
fn daemon_url(config_path: Option<&str>) -> Option<String> {
    let config = load_config(config_path).ok()?;
    let url = format!("http://{}", config.api_listen);
    let health_url = format!("{}/health", url);

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

/// Resolve a fighter name to its UUID via the API.
async fn resolve_fighter_id(
    client: &reqwest::Client,
    base_url: &str,
    name_or_id: &str,
) -> Option<String> {
    // If it looks like a UUID already, return it.
    if uuid::Uuid::parse_str(name_or_id).is_ok() {
        return Some(name_or_id.to_string());
    }

    // Look up by name via the fighters list endpoint.
    let url = format!("{}/api/fighters", base_url);
    let resp = client.get(&url).send().await.ok()?;
    let fighters: Vec<serde_json::Value> = resp.json().await.ok()?;

    for f in &fighters {
        if f["name"].as_str() == Some(name_or_id) {
            return f["id"].as_str().map(|s| s.to_string());
        }
    }
    None
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${:.4}", cost)
    } else {
        format!("${:.2}", cost)
    }
}

fn print_global_stats(stats: &serde_json::Value) {
    let period = stats["period"].as_str().unwrap_or("day");
    let total_in = stats["total_input_tokens"].as_u64().unwrap_or(0);
    let total_out = stats["total_output_tokens"].as_u64().unwrap_or(0);
    let total_cost = stats["total_cost_usd"].as_f64().unwrap_or(0.0);
    let total_reqs = stats["total_requests"].as_u64().unwrap_or(0);

    println!();
    println!("  Token Usage Stats (period: {period})");
    println!("  {}", "=".repeat(60));
    println!();
    println!(
        "  Total:  {} in / {} out  |  {} tokens  |  {}  |  {} requests",
        format_tokens(total_in),
        format_tokens(total_out),
        format_tokens(total_in + total_out),
        format_cost(total_cost),
        total_reqs
    );

    // By model.
    if let Some(models) = stats["by_model"].as_array()
        && !models.is_empty()
    {
        println!();
        println!(
            "  {:<35}  {:>10}  {:>10}  {:>10}  {:>6}",
            "MODEL", "INPUT", "OUTPUT", "COST", "REQS"
        );
        println!("  {}", "-".repeat(75));
        for m in models {
            println!(
                "  {:<35}  {:>10}  {:>10}  {:>10}  {:>6}",
                m["model"].as_str().unwrap_or("-"),
                format_tokens(m["input_tokens"].as_u64().unwrap_or(0)),
                format_tokens(m["output_tokens"].as_u64().unwrap_or(0)),
                format_cost(m["cost_usd"].as_f64().unwrap_or(0.0)),
                m["request_count"].as_u64().unwrap_or(0),
            );
        }
    }

    // By fighter.
    if let Some(fighters) = stats["by_fighter"].as_array()
        && !fighters.is_empty()
    {
        println!();
        println!(
            "  {:<24}  {:>10}  {:>10}  {:>10}  {:>6}",
            "FIGHTER", "INPUT", "OUTPUT", "COST", "REQS"
        );
        println!("  {}", "-".repeat(64));
        for f in fighters {
            println!(
                "  {:<24}  {:>10}  {:>10}  {:>10}  {:>6}",
                f["fighter_name"].as_str().unwrap_or("-"),
                format_tokens(f["input_tokens"].as_u64().unwrap_or(0)),
                format_tokens(f["output_tokens"].as_u64().unwrap_or(0)),
                format_cost(f["cost_usd"].as_f64().unwrap_or(0.0)),
                f["request_count"].as_u64().unwrap_or(0),
            );
        }
    }

    println!();
}

fn print_fighter_stats(stats: &serde_json::Value) {
    let name = stats["fighter_name"].as_str().unwrap_or("unknown");
    let period = stats["period"].as_str().unwrap_or("day");
    let total_in = stats["total_input_tokens"].as_u64().unwrap_or(0);
    let total_out = stats["total_output_tokens"].as_u64().unwrap_or(0);
    let total_cost = stats["total_cost_usd"].as_f64().unwrap_or(0.0);
    let total_reqs = stats["total_requests"].as_u64().unwrap_or(0);

    println!();
    println!("  Token Usage Stats for \"{name}\" (period: {period})");
    println!("  {}", "=".repeat(60));
    println!();
    println!(
        "  Total:  {} in / {} out  |  {} tokens  |  {}  |  {} requests",
        format_tokens(total_in),
        format_tokens(total_out),
        format_tokens(total_in + total_out),
        format_cost(total_cost),
        total_reqs
    );

    // By model.
    if let Some(models) = stats["by_model"].as_array()
        && !models.is_empty()
    {
        println!();
        println!(
            "  {:<35}  {:>10}  {:>10}  {:>10}  {:>6}",
            "MODEL", "INPUT", "OUTPUT", "COST", "REQS"
        );
        println!("  {}", "-".repeat(75));
        for m in models {
            println!(
                "  {:<35}  {:>10}  {:>10}  {:>10}  {:>6}",
                m["model"].as_str().unwrap_or("-"),
                format_tokens(m["input_tokens"].as_u64().unwrap_or(0)),
                format_tokens(m["output_tokens"].as_u64().unwrap_or(0)),
                format_cost(m["cost_usd"].as_f64().unwrap_or(0.0)),
                m["request_count"].as_u64().unwrap_or(0),
            );
        }
    }

    println!();
}
