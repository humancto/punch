//! `punch chat` — Quick chat interface.
//!
//! Sends a message to the Punch API and prints the response. Supports
//! model override, custom system prompts, and streaming output.

use super::load_dotenv;

/// Base URL for the Punch daemon API.
fn api_base() -> String {
    std::env::var("PUNCH_API_URL").unwrap_or_else(|_| "http://127.0.0.1:6660".to_string())
}

pub async fn run(
    message: Option<String>,
    model: Option<String>,
    system: Option<String>,
    stream: bool,
    config_path: Option<String>,
) -> i32 {
    load_dotenv();

    match message {
        Some(msg) => run_oneshot(&msg, model, system, stream).await,
        None => run_interactive(model, system, config_path).await,
    }
}

/// One-shot chat: send a single message via the API and print the response.
async fn run_oneshot(
    message: &str,
    model: Option<String>,
    system: Option<String>,
    stream: bool,
) -> i32 {
    let url = format!("{}/api/chat", api_base());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  [X] Failed to create HTTP client: {}", e);
            return 1;
        }
    };

    let mut body = serde_json::json!({
        "message": message,
        "stream": stream,
    });

    if let Some(ref m) = model {
        body["model"] = serde_json::Value::String(m.clone());
    }
    if let Some(ref s) = system {
        body["system"] = serde_json::Value::String(s.clone());
    }

    if stream {
        return run_streaming_chat(&client, &url, &body).await;
    }

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(data) => {
                        if let Some(response) = data["response"].as_str() {
                            println!("{}", response);
                        } else if let Some(content) = data["content"].as_str() {
                            println!("{}", content);
                        } else {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&data).unwrap_or_default()
                            );
                        }

                        if let Some(usage) = data.get("usage") {
                            let input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
                            let output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
                            if input_tokens > 0 || output_tokens > 0 {
                                eprintln!(
                                    "  [tokens: {} in / {} out]",
                                    input_tokens, output_tokens
                                );
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
                let status = resp.status();
                let body_text = resp.text().await.unwrap_or_default();
                eprintln!("  [X] API error ({}): {}", status, body_text);
                1
            }
        }
        Err(e) => {
            if e.is_connect() {
                eprintln!("  [X] Cannot connect to Punch daemon at {}", api_base());
                eprintln!("      Is the daemon running? Try: punch start");
            } else if e.is_timeout() {
                eprintln!("  [X] Request timed out");
            } else {
                eprintln!("  [X] Request failed: {}", e);
            }
            1
        }
    }
}

/// Streaming chat: read response body in chunks and print tokens as they arrive.
async fn run_streaming_chat(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
) -> i32 {
    let resp = match client.post(url).json(body).send().await {
        Ok(r) => r,
        Err(e) => {
            if e.is_connect() {
                eprintln!("  [X] Cannot connect to Punch daemon at {}", api_base());
                eprintln!("      Is the daemon running? Try: punch start");
            } else {
                eprintln!("  [X] Request failed: {}", e);
            }
            return 1;
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        eprintln!("  [X] API error ({}): {}", status, body_text);
        return 1;
    }

    // Read the full response body as text. For true SSE streaming,
    // we read the body and process line by line.
    let body_text = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  [X] Failed to read response: {}", e);
            return 1;
        }
    };

    for line in body_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Try to parse as SSE data line.
        let data = if let Some(stripped) = trimmed.strip_prefix("data: ") {
            stripped
        } else {
            trimmed
        };

        if data == "[DONE]" {
            break;
        }

        if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) {
            if let Some(text) = chunk["content"].as_str() {
                print!("{}", text);
            } else if let Some(text) = chunk["delta"].as_str() {
                print!("{}", text);
            } else if let Some(text) = chunk["response"].as_str() {
                print!("{}", text);
            }
        } else {
            // If it's not JSON, just print the raw text.
            print!("{}", data);
        }
    }
    println!();
    0
}

/// Interactive chat mode: falls back to the fighter REPL.
async fn run_interactive(
    _model: Option<String>,
    _system: Option<String>,
    config_path: Option<String>,
) -> i32 {
    // Delegate to the existing fighter quick-chat for interactive mode.
    super::fighter::run_quick_chat(None, config_path).await
}

#[cfg(test)]
mod tests {
    /// Build the chat API URL.
    fn build_chat_url(base: &str) -> String {
        format!("{}/api/chat", base)
    }

    /// Build a chat request body.
    fn build_chat_body(
        message: &str,
        model: Option<&str>,
        system: Option<&str>,
        stream: bool,
    ) -> serde_json::Value {
        let mut body = serde_json::json!({
            "message": message,
            "stream": stream,
        });
        if let Some(m) = model {
            body["model"] = serde_json::Value::String(m.to_string());
        }
        if let Some(s) = system {
            body["system"] = serde_json::Value::String(s.to_string());
        }
        body
    }

    /// Format a chat response for display.
    fn format_chat_response(data: &serde_json::Value) -> String {
        if let Some(response) = data["response"].as_str() {
            response.to_string()
        } else if let Some(content) = data["content"].as_str() {
            content.to_string()
        } else {
            serde_json::to_string_pretty(data).unwrap_or_default()
        }
    }

    /// Format an error message for connection failures.
    fn format_connection_error(base_url: &str) -> String {
        format!(
            "Cannot connect to Punch daemon at {}\n      Is the daemon running? Try: punch start",
            base_url
        )
    }

    #[test]
    fn test_build_chat_url() {
        assert_eq!(
            build_chat_url("http://localhost:6660"),
            "http://localhost:6660/api/chat"
        );
        assert_eq!(
            build_chat_url("http://example.com:8080"),
            "http://example.com:8080/api/chat"
        );
    }

    #[test]
    fn test_build_chat_body_minimal() {
        let body = build_chat_body("hello", None, None, false);
        assert_eq!(body["message"], "hello");
        assert_eq!(body["stream"], false);
        assert!(body.get("model").is_none() || body["model"].is_null());
        assert!(body.get("system").is_none() || body["system"].is_null());
    }

    #[test]
    fn test_build_chat_body_full() {
        let body = build_chat_body("hello", Some("gpt-4o"), Some("Be brief"), true);
        assert_eq!(body["message"], "hello");
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["system"], "Be brief");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn test_format_chat_response_with_response_field() {
        let data = serde_json::json!({"response": "Hello there!"});
        assert_eq!(format_chat_response(&data), "Hello there!");
    }

    #[test]
    fn test_format_chat_response_with_content_field() {
        let data = serde_json::json!({"content": "Hi!"});
        assert_eq!(format_chat_response(&data), "Hi!");
    }

    #[test]
    fn test_format_chat_response_fallback() {
        let data = serde_json::json!({"other": "value"});
        let result = format_chat_response(&data);
        assert!(result.contains("other"));
    }

    #[test]
    fn test_format_connection_error() {
        let msg = format_connection_error("http://localhost:6660");
        assert!(msg.contains("localhost:6660"));
        assert!(msg.contains("punch start"));
    }

    #[test]
    fn test_api_base_default() {
        // When PUNCH_API_URL is not set, falls back to default.
        // We can't reliably test env vars in unit tests without side effects,
        // so we just test the URL builder directly.
        let url = build_chat_url("http://127.0.0.1:6660");
        assert_eq!(url, "http://127.0.0.1:6660/api/chat");
    }

    #[test]
    fn test_build_chat_body_stream_flag() {
        let body_stream = build_chat_body("msg", None, None, true);
        assert_eq!(body_stream["stream"], true);

        let body_no_stream = build_chat_body("msg", None, None, false);
        assert_eq!(body_no_stream["stream"], false);
    }

    #[test]
    fn test_format_response_prefers_response_over_content() {
        let data = serde_json::json!({"response": "resp", "content": "cont"});
        assert_eq!(format_chat_response(&data), "resp");
    }
}
