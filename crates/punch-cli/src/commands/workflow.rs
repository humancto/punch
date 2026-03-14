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
        WorkflowCommands::Create { file } => run_create(&file).await,
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
                                println!("{:<38} {:<30} STEPS", "ID", "NAME");
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
            if let Ok(resp) = client.get(&run_url).send().await
                && resp.status().is_success()
                && let Ok(run) = resp.json::<serde_json::Value>().await
            {
                println!("Run ID:      {}", run["id"].as_str().unwrap_or("?"));
                println!("Workflow:    {}", wf_id);
                println!("Status:      {}", run["status"].as_str().unwrap_or("?"));
                println!("Started:     {}", run["started_at"].as_str().unwrap_or("?"));
                if let Some(completed) = run["completed_at"].as_str() {
                    println!("Completed:   {}", completed);
                }

                if let Some(steps) = run["step_results"].as_array() {
                    println!();
                    println!("Steps:");
                    for (i, step) in steps.iter().enumerate() {
                        let name = step["step_name"].as_str().unwrap_or("?");
                        let tokens = step["tokens_used"].as_u64().unwrap_or(0);
                        let duration = step["duration_ms"].as_u64().unwrap_or(0);
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

    eprintln!("  [X] Run {} not found", run_id);
    1
}

async fn run_create(file_path: &str) -> i32 {
    let path = std::path::Path::new(file_path);

    if !path.exists() {
        eprintln!("  [X] File not found: {}", file_path);
        return 1;
    }

    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] Failed to read file: {}", e);
            return 1;
        }
    };

    // Parse the workflow definition — support both TOML and JSON.
    let workflow_def: serde_json::Value = if file_path.ends_with(".toml") {
        match toml::from_str::<toml::Value>(&contents) {
            Ok(toml_val) => {
                // Convert TOML to JSON for the API call.
                let json_str = match serde_json::to_string(&toml_val) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("  [X] Failed to convert TOML to JSON: {}", e);
                        return 1;
                    }
                };
                match serde_json::from_str(&json_str) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("  [X] Failed to parse converted JSON: {}", e);
                        return 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("  [X] Failed to parse TOML: {}", e);
                return 1;
            }
        }
    } else {
        match serde_json::from_str(&contents) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  [X] Failed to parse JSON: {}", e);
                return 1;
            }
        }
    };

    let url = format!("{}/api/workflows", api_base());
    let client = reqwest::Client::new();

    println!("  Creating workflow from {}...", file_path);

    match client.post(&url).json(&workflow_def).send().await {
        Ok(resp) => {
            if resp.status().is_success() || resp.status() == reqwest::StatusCode::CREATED {
                match resp.json::<serde_json::Value>().await {
                    Ok(result) => {
                        println!();
                        println!(
                            "  Workflow created: {}",
                            result["name"].as_str().unwrap_or("?")
                        );
                        println!("  ID: {}", result["id"].as_str().unwrap_or("?"));
                        println!("  Steps: {}", result["step_count"].as_u64().unwrap_or(0));
                        println!();
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
            eprintln!("      Is the daemon running? Try: punch start");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    /// Build the workflow list URL.
    fn build_list_url(base: &str) -> String {
        format!("{}/api/workflows", base)
    }

    /// Build the workflow execute URL.
    fn build_execute_url(base: &str, name: &str) -> String {
        format!("{}/api/workflows/{}/execute", base, name)
    }

    /// Build the workflow run status URL.
    fn build_run_status_url(base: &str, wf_id: &str, run_id: &str) -> String {
        format!("{}/api/workflows/{}/runs/{}", base, wf_id, run_id)
    }

    /// Build the workflow create URL.
    fn build_create_url(base: &str) -> String {
        format!("{}/api/workflows", base)
    }

    /// Format a workflows table.
    fn format_workflows_table(workflows: &[serde_json::Value]) -> String {
        let mut lines = Vec::new();
        lines.push(format!("{:<38} {:<30} STEPS", "ID", "NAME"));
        lines.push("-".repeat(75));

        for w in workflows {
            lines.push(format!(
                "{:<38} {:<30} {}",
                w["id"].as_str().unwrap_or("?"),
                w["name"].as_str().unwrap_or("?"),
                w["step_count"].as_u64().unwrap_or(0),
            ));
        }

        lines.join("\n")
    }

    /// Format a workflow run status display.
    fn format_run_status(run: &serde_json::Value) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Run ID:      {}",
            run["id"].as_str().unwrap_or("?")
        ));
        lines.push(format!(
            "Status:      {}",
            run["status"].as_str().unwrap_or("?")
        ));
        lines.push(format!(
            "Started:     {}",
            run["started_at"].as_str().unwrap_or("?")
        ));
        if let Some(completed) = run["completed_at"].as_str() {
            lines.push(format!("Completed:   {}", completed));
        }
        lines.join("\n")
    }

    #[test]
    fn test_build_list_url() {
        assert_eq!(
            build_list_url("http://localhost:6660"),
            "http://localhost:6660/api/workflows"
        );
    }

    #[test]
    fn test_build_execute_url() {
        assert_eq!(
            build_execute_url("http://localhost:6660", "research-and-summarize"),
            "http://localhost:6660/api/workflows/research-and-summarize/execute"
        );
    }

    #[test]
    fn test_build_run_status_url() {
        assert_eq!(
            build_run_status_url("http://localhost:6660", "wf-1", "run-abc"),
            "http://localhost:6660/api/workflows/wf-1/runs/run-abc"
        );
    }

    #[test]
    fn test_build_create_url() {
        assert_eq!(
            build_create_url("http://localhost:6660"),
            "http://localhost:6660/api/workflows"
        );
    }

    #[test]
    fn test_format_workflows_table_empty() {
        let table = format_workflows_table(&[]);
        assert!(table.contains("ID"));
        assert!(table.contains("NAME"));
        assert!(table.contains("STEPS"));
    }

    #[test]
    fn test_format_workflows_table_with_data() {
        let workflows = vec![serde_json::json!({
            "id": "wf-123",
            "name": "research-and-summarize",
            "step_count": 2,
        })];
        let table = format_workflows_table(&workflows);
        assert!(table.contains("research-and-summarize"));
        assert!(table.contains("wf-123"));
    }

    #[test]
    fn test_format_run_status() {
        let run = serde_json::json!({
            "id": "run-abc",
            "status": "completed",
            "started_at": "2024-01-01T00:00:00Z",
            "completed_at": "2024-01-01T00:01:00Z",
        });
        let output = format_run_status(&run);
        assert!(output.contains("run-abc"));
        assert!(output.contains("completed"));
    }

    #[test]
    fn test_format_run_status_no_completion() {
        let run = serde_json::json!({
            "id": "run-abc",
            "status": "running",
            "started_at": "2024-01-01T00:00:00Z",
        });
        let output = format_run_status(&run);
        assert!(output.contains("running"));
        assert!(!output.contains("Completed:"));
    }

    #[test]
    fn test_execute_body() {
        let body = serde_json::json!({ "input": "test input" });
        assert_eq!(body["input"], "test input");
    }

    #[test]
    fn test_workflow_response_parsing() {
        let result = serde_json::json!({
            "run_id": "run-xyz",
            "status": "started",
        });
        assert_eq!(result["run_id"].as_str().unwrap(), "run-xyz");
        assert_eq!(result["status"].as_str().unwrap(), "started");
    }

    #[test]
    fn test_step_results_parsing() {
        let run = serde_json::json!({
            "step_results": [
                {"step_name": "scout", "tokens_used": 1500, "duration_ms": 3000},
                {"step_name": "oracle", "tokens_used": 800, "duration_ms": 2000, "error": "timeout"},
            ]
        });
        let steps = run["step_results"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["step_name"].as_str().unwrap(), "scout");
        assert!(steps[1]["error"].is_string());
    }
}
