//! Tool execution engine.
//!
//! Executes built-in tools (moves) with capability checking, timeout
//! enforcement, and SSRF protection for network-facing tools.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::process::Command;
use tracing::{debug, instrument};

use punch_memory::MemorySubstrate;
use punch_types::{
    Capability, FighterId, PunchError, PunchResult, ToolResult, capability::capability_matches,
};

/// Context passed to every tool execution.
pub struct ToolExecutionContext {
    /// Working directory for filesystem and shell operations.
    pub working_dir: PathBuf,
    /// The fighter invoking the tool.
    pub fighter_id: FighterId,
    /// Memory substrate for memory/knowledge tools.
    pub memory: Arc<MemorySubstrate>,
}

/// Default per-tool timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Execute a tool by name with the given input, checking capabilities first.
///
/// Returns a [`ToolResult`] with success/failure status and output.
#[instrument(skip(input, capabilities, context), fields(tool = %name, fighter = %context.fighter_id))]
pub async fn execute_tool(
    name: &str,
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let start = Instant::now();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        execute_tool_inner(name, input, capabilities, context),
    )
    .await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(mut tool_result)) => {
            tool_result.duration_ms = duration_ms;
            Ok(tool_result)
        }
        Ok(Err(e)) => Ok(ToolResult {
            success: false,
            output: serde_json::json!(null),
            error: Some(e.to_string()),
            duration_ms,
        }),
        Err(_) => Err(PunchError::ToolTimeout {
            tool: name.to_string(),
            timeout_ms: DEFAULT_TIMEOUT_SECS * 1000,
        }),
    }
}

/// Inner dispatch without timeout wrapper.
async fn execute_tool_inner(
    name: &str,
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    match name {
        "file_read" => tool_file_read(input, capabilities, context).await,
        "file_write" => tool_file_write(input, capabilities, context).await,
        "file_list" => tool_file_list(input, capabilities, context).await,
        "shell_exec" => tool_shell_exec(input, capabilities, context).await,
        "web_search" => tool_web_search(input).await,
        "web_fetch" => tool_web_fetch(input, capabilities).await,
        "memory_store" => tool_memory_store(input, capabilities, context).await,
        "memory_recall" => tool_memory_recall(input, capabilities, context).await,
        "knowledge_add_entity" => tool_knowledge_add_entity(input, capabilities, context).await,
        "knowledge_add_relation" => tool_knowledge_add_relation(input, capabilities, context).await,
        "knowledge_query" => tool_knowledge_query(input, capabilities, context).await,
        _ => Err(PunchError::ToolNotFound(name.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Capability helpers
// ---------------------------------------------------------------------------

/// Check that at least one granted capability satisfies the requirement.
fn require_capability(capabilities: &[Capability], required: &Capability) -> PunchResult<()> {
    if capabilities
        .iter()
        .any(|granted| capability_matches(granted, required))
    {
        Ok(())
    } else {
        Err(PunchError::CapabilityDenied(format!(
            "missing capability: {}",
            required
        )))
    }
}

/// Resolve a path relative to the working directory.
fn resolve_path(working_dir: &Path, requested: &str) -> PunchResult<PathBuf> {
    let path = if Path::new(requested).is_absolute() {
        PathBuf::from(requested)
    } else {
        working_dir.join(requested)
    };

    Ok(path)
}

// ---------------------------------------------------------------------------
// Built-in tool implementations
// ---------------------------------------------------------------------------

async fn tool_file_read(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let path_str = input["path"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "file_read".into(),
        message: "missing 'path' parameter".into(),
    })?;

    let path = resolve_path(&context.working_dir, path_str)?;
    let path_display = path.display().to_string();

    require_capability(capabilities, &Capability::FileRead(path_display.clone()))?;

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            debug!(path = %path_display, bytes = content.len(), "file read");
            Ok(ToolResult {
                success: true,
                output: serde_json::json!(content),
                error: None,
                duration_ms: 0,
            })
        }
        Err(e) => Ok(ToolResult {
            success: false,
            output: serde_json::json!(null),
            error: Some(format!("failed to read '{}': {}", path_display, e)),
            duration_ms: 0,
        }),
    }
}

async fn tool_file_write(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let path_str = input["path"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "file_write".into(),
        message: "missing 'path' parameter".into(),
    })?;
    let content = input["content"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "file_write".into(),
        message: "missing 'content' parameter".into(),
    })?;

    let path = resolve_path(&context.working_dir, path_str)?;
    let path_display = path.display().to_string();

    require_capability(capabilities, &Capability::FileWrite(path_display.clone()))?;

    // Ensure parent directory exists.
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| PunchError::Tool {
                tool: "file_write".into(),
                message: format!("failed to create directory '{}': {}", parent.display(), e),
            })?;
    }

    match tokio::fs::write(&path, content).await {
        Ok(()) => {
            debug!(path = %path_display, bytes = content.len(), "file written");
            Ok(ToolResult {
                success: true,
                output: serde_json::json!(format!(
                    "wrote {} bytes to {}",
                    content.len(),
                    path_display
                )),
                error: None,
                duration_ms: 0,
            })
        }
        Err(e) => Ok(ToolResult {
            success: false,
            output: serde_json::json!(null),
            error: Some(format!("failed to write '{}': {}", path_display, e)),
            duration_ms: 0,
        }),
    }
}

async fn tool_file_list(
    input: &serde_json::Value,
    _capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let path_str = input["path"].as_str().unwrap_or(".");
    let path = resolve_path(&context.working_dir, path_str)?;

    let mut entries = Vec::new();
    let mut dir = tokio::fs::read_dir(&path)
        .await
        .map_err(|e| PunchError::Tool {
            tool: "file_list".into(),
            message: format!("failed to list '{}': {}", path.display(), e),
        })?;

    while let Some(entry) = dir.next_entry().await.map_err(|e| PunchError::Tool {
        tool: "file_list".into(),
        message: format!("failed to read entry: {}", e),
    })? {
        let file_type = entry.file_type().await.ok();
        let is_dir = file_type.as_ref().map(|ft| ft.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        entries.push(serde_json::json!({
            "name": name,
            "is_directory": is_dir,
        }));
    }

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(entries),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_shell_exec(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let command_str = input["command"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "shell_exec".into(),
        message: "missing 'command' parameter".into(),
    })?;

    require_capability(
        capabilities,
        &Capability::ShellExec(command_str.to_string()),
    )?;

    // Note: Shell execution is capability-gated. The command string comes from
    // the LLM and is validated via the ShellExec capability pattern before
    // execution. This is intentional for an agent runtime that needs to run
    // arbitrary commands on behalf of the user.
    let output = Command::new("sh")
        .arg("-c")
        .arg(command_str)
        .current_dir(&context.working_dir)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "shell_exec".into(),
            message: format!("failed to execute command: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    debug!(exit_code = exit_code, "shell exec complete");

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
        }),
        error: if output.status.success() {
            None
        } else {
            Some(format!("command exited with code {}", exit_code))
        },
        duration_ms: 0,
    })
}

async fn tool_web_search(_input: &serde_json::Value) -> PunchResult<ToolResult> {
    Ok(ToolResult {
        success: false,
        output: serde_json::json!("web search not yet configured"),
        error: Some("web search not yet configured".to_string()),
        duration_ms: 0,
    })
}

async fn tool_web_fetch(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    let url_str = input["url"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "web_fetch".into(),
        message: "missing 'url' parameter".into(),
    })?;

    let parsed_url = url::Url::parse(url_str).map_err(|e| PunchError::Tool {
        tool: "web_fetch".into(),
        message: format!("invalid URL: {}", e),
    })?;

    // SSRF protection: block private/loopback IPs.
    if let Some(host) = parsed_url.host_str() {
        require_capability(capabilities, &Capability::Network(host.to_string()))?;

        if let Ok(ip) = host.parse::<IpAddr>()
            && is_private_ip(&ip)
        {
            return Ok(ToolResult {
                success: false,
                output: serde_json::json!(null),
                error: Some(format!(
                    "SSRF protection: requests to private IP {} are blocked",
                    ip
                )),
                duration_ms: 0,
            });
        }

        // Also check resolved addresses for hostnames.
        if let Ok(addrs) = tokio::net::lookup_host(format!("{}:80", host)).await {
            for addr in addrs {
                if is_private_ip(&addr.ip()) {
                    return Ok(ToolResult {
                        success: false,
                        output: serde_json::json!(null),
                        error: Some(format!(
                            "SSRF protection: hostname '{}' resolves to private IP {}",
                            host,
                            addr.ip()
                        )),
                        duration_ms: 0,
                    });
                }
            }
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| PunchError::Tool {
            tool: "web_fetch".into(),
            message: format!("failed to create HTTP client: {}", e),
        })?;

    let response = client
        .get(url_str)
        .send()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "web_fetch".into(),
            message: format!("request failed: {}", e),
        })?;

    let status = response.status().as_u16();
    let body = response.text().await.map_err(|e| PunchError::Tool {
        tool: "web_fetch".into(),
        message: format!("failed to read response body: {}", e),
    })?;

    // Truncate very large responses.
    let truncated = if body.len() > 100_000 {
        format!(
            "{}... [truncated, {} total bytes]",
            &body[..100_000],
            body.len()
        )
    } else {
        body
    };

    Ok(ToolResult {
        success: (200..300).contains(&(status as usize)),
        output: serde_json::json!({
            "status": status,
            "body": truncated,
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_memory_store(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Memory)?;

    let key = input["key"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "memory_store".into(),
        message: "missing 'key' parameter".into(),
    })?;
    let value = input["value"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "memory_store".into(),
        message: "missing 'value' parameter".into(),
    })?;
    let confidence = input["confidence"].as_f64().unwrap_or(0.9);

    context
        .memory
        .store_memory(&context.fighter_id, key, value, confidence)
        .await?;

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(format!("stored memory '{}'", key)),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_memory_recall(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Memory)?;

    let query = input["query"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "memory_recall".into(),
        message: "missing 'query' parameter".into(),
    })?;
    let limit = input["limit"].as_u64().unwrap_or(10) as u32;

    let memories = context
        .memory
        .recall_memories(&context.fighter_id, query, limit)
        .await?;

    let entries: Vec<serde_json::Value> = memories
        .iter()
        .map(|m| {
            serde_json::json!({
                "key": m.key,
                "value": m.value,
                "confidence": m.confidence,
            })
        })
        .collect();

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(entries),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_knowledge_add_entity(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::KnowledgeGraph)?;

    let name = input["name"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "knowledge_add_entity".into(),
        message: "missing 'name' parameter".into(),
    })?;
    let entity_type = input["entity_type"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "knowledge_add_entity".into(),
            message: "missing 'entity_type' parameter".into(),
        })?;
    let properties = input
        .get("properties")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    context
        .memory
        .add_entity(&context.fighter_id, name, entity_type, &properties)
        .await?;

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(format!("added entity '{}' ({})", name, entity_type)),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_knowledge_add_relation(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::KnowledgeGraph)?;

    let from = input["from"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "knowledge_add_relation".into(),
        message: "missing 'from' parameter".into(),
    })?;
    let relation = input["relation"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "knowledge_add_relation".into(),
        message: "missing 'relation' parameter".into(),
    })?;
    let to = input["to"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "knowledge_add_relation".into(),
        message: "missing 'to' parameter".into(),
    })?;
    let properties = input
        .get("properties")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    context
        .memory
        .add_relation(&context.fighter_id, from, relation, to, &properties)
        .await?;

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(format!("{} --[{}]--> {}", from, relation, to)),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_knowledge_query(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::KnowledgeGraph)?;

    let query = input["query"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "knowledge_query".into(),
        message: "missing 'query' parameter".into(),
    })?;

    let entities = context
        .memory
        .query_entities(&context.fighter_id, query)
        .await?;

    let entity_results: Vec<serde_json::Value> = entities
        .iter()
        .map(|e| {
            serde_json::json!({
                "name": e.name,
                "type": e.entity_type,
                "properties": e.properties,
            })
        })
        .collect();

    // Also query relations for any matched entity.
    let mut all_relations = Vec::new();
    for entity in &entities {
        let relations = context
            .memory
            .query_relations(&context.fighter_id, &entity.name)
            .await?;
        for r in relations {
            all_relations.push(serde_json::json!({
                "from": r.from_entity,
                "relation": r.relation,
                "to": r.to_entity,
                "properties": r.properties,
            }));
        }
    }

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "entities": entity_results,
            "relations": all_relations,
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// SSRF protection
// ---------------------------------------------------------------------------

/// Check if an IP address is in a private/reserved range.
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()           // 127.0.0.0/8
                || v4.is_private()     // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()  // 169.254/16
                || v4.is_broadcast()   // 255.255.255.255
                || v4.is_unspecified() // 0.0.0.0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()           // ::1
                || v6.is_unspecified() // ::
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::Capability;

    #[test]
    fn test_require_capability_granted() {
        let caps = vec![Capability::FileRead("**".to_string())];
        assert!(
            require_capability(&caps, &Capability::FileRead("src/main.rs".to_string())).is_ok()
        );
    }

    #[test]
    fn test_require_capability_denied() {
        let caps = vec![Capability::Memory];
        let result = require_capability(&caps, &Capability::FileRead("src/main.rs".to_string()));
        assert!(result.is_err());
        match result.unwrap_err() {
            PunchError::CapabilityDenied(msg) => {
                assert!(msg.contains("file_read"));
            }
            other => panic!("expected CapabilityDenied, got {:?}", other),
        }
    }

    #[test]
    fn test_require_capability_scoped_match() {
        let caps = vec![Capability::FileRead("src/**/*.rs".to_string())];
        assert!(require_capability(&caps, &Capability::FileRead("src/lib.rs".to_string())).is_ok());
        assert!(
            require_capability(&caps, &Capability::FileRead("tests/foo.rs".to_string())).is_err()
        );
    }

    #[test]
    fn test_require_capability_shell_wildcard() {
        let caps = vec![Capability::ShellExec("*".to_string())];
        assert!(require_capability(&caps, &Capability::ShellExec("ls -la".to_string())).is_ok());
    }

    #[test]
    fn test_is_private_ip() {
        assert!(is_private_ip(&"127.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"192.168.1.1".parse().unwrap()));
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(is_private_ip(&"::1".parse().unwrap()));
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(&"1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn test_require_network_capability() {
        let caps = vec![Capability::Network("*.example.com".to_string())];
        assert!(
            require_capability(&caps, &Capability::Network("api.example.com".to_string())).is_ok()
        );
        assert!(require_capability(&caps, &Capability::Network("evil.com".to_string())).is_err());
    }
}
