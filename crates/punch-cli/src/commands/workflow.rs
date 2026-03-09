//! `punch workflow` — Manage multi-step agent workflows.

use crate::cli::WorkflowCommands;

/// Base URL for the Punch daemon API.
fn api_base() -> String {
    std::env::var("PUNCH_API_URL").unwrap_or_else(|_| "http://127.0.0.1:6660".to_string())
}

pub async fn run(command: WorkflowCommands) -> i32 {
    match command {
        WorkflowCommands::List => run_list().await,
        WorkflowCommands::Run { id, input } => run_execute(&id, &input).await,
        WorkflowCommands::Status { run_id } => run_status(&run_id).await,
    }
}

async fn run_list() -> i32 {
    let url = format!("{}/api/workflows", api_base());
    let client = reqwest::Client::new();

    match client.get(&url).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(workflows) => {
                        if let Some(arr) = workflows.as_array() {
                            if arr.is_empty() {
                                println!("No workflows registered.");
                            } else {
                                println!("{:<38} {:<30} {}", "ID", "NAME", "STEPS");
                                println!("{}", "-".repeat(75));
                                for w in arr {
                                    println!(
                                        "{:<38} {:<30} {}",
                                        w["id"].as_str().unwrap_or("?"),
                                        w["name"].as_str().unwrap_or("?"),
                                        w["step_count"].as_u64().unwrap_or(0),
                                    );
                                }
                            }
                        }
                        0
                    }
                    Err(e) => {
                        eprintln!("  [X] Failed to parse response: {}", e);
                        1
                    }
                }
            } else {
                eprintln!("  [X] API error: {}", resp.status());
                1
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to connect to daemon: {}", e);
            eprintln!("      Is the daemon running? Try: punch start");
            1
        }
    }
}

async fn run_execute(id: &str, input: &str) -> i32 {
    let url = format!("{}/api/workflows/{}/execute", api_base(), id);
    let client = reqwest::Client::new();

    let body = serde_json::json!({ "input": input });

    println!("Executing workflow {}...", id);

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(result) => {
                        println!("Run ID:  {}", result["run_id"].as_str().unwrap_or("?"));
                        println!("Status:  {}", result["status"].as_str().unwrap_or("?"));
                        0
                    }
                    Err(e) => {
                        eprintln!("  [X] Failed to parse response: {}", e);
                        1
                    }
                }
            } else {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                eprintln!("  [X] API error ({}): {}", status, body);
                1
            }
        }
        Err(e) => {
            eprintln!("  [X] Failed to connect to daemon: {}", e);
            1
        }
    }
}

async fn run_status(run_id: &str) -> i32 {
    // We need to search across all workflows for this run_id.
    // First list all workflows, then check runs for each, or use a direct get.
    // The API requires /api/workflows/:id/runs/:run_id, but we don't know the
    // workflow ID. We'll try listing all workflows and checking.
    let base = api_base();
    let client = reqwest::Client::new();

    // List workflows first
    let workflows_url = format!("{}/api/workflows", base);
    let workflows_resp = match client.get(&workflows_url).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  [X] Failed to connect to daemon: {}", e);
            return 1;
        }
    };

    let workflows: Vec<serde_json::Value> = match workflows_resp.json().await {
        Ok(w) => w,
        Err(e) => {
            eprintln!("  [X] Failed to parse workflows: {}", e);
            return 1;
        }
    };

    // Try each workflow to find the run
    for w in &workflows {
        if let Some(wf_id) = w["id"].as_str() {
            let run_url = format!("{}/api/workflows/{}/runs/{}", base, wf_id, run_id);
            if let Ok(resp) = client.get(&run_url).send().await {
                if resp.status().is_success() {
                    if let Ok(run) = resp.json::<serde_json::Value>().await {
                        println!("Run ID:      {}", run["id"].as_str().unwrap_or("?"));
                        println!("Workflow:    {}", wf_id);
                        println!(
                            "Status:      {}",
                            run["status"].as_str().unwrap_or("?")
                        );
                        println!(
                            "Started:     {}",
                            run["started_at"].as_str().unwrap_or("?")
                        );
                        if let Some(completed) = run["completed_at"].as_str() {
                            println!("Completed:   {}", completed);
                        }

                        if let Some(steps) = run["step_results"].as_array() {
                            println!();
                            println!("Steps:");
                            for (i, step) in steps.iter().enumerate() {
                                let name =
                                    step["step_name"].as_str().unwrap_or("?");
                                let tokens = step["tokens_used"].as_u64().unwrap_or(0);
                                let duration =
                                    step["duration_ms"].as_u64().unwrap_or(0);
                                let has_error = step["error"].is_string();
                                let status_icon = if has_error { "X" } else { "+" };
                                println!(
                                    "  [{}] Step {} ({}): {}ms, {} tokens",
                                    status_icon,
                                    i + 1,
                                    name,
                                    duration,
                                    tokens,
                                );
                                if let Some(err) = step["error"].as_str() {
                                    println!("      Error: {}", err);
                                }
                            }
                        }

                        return 0;
                    }
                }
            }
        }
    }

    eprintln!("  [X] Run {} not found", run_id);
    1
}
