//! Tool execution engine.
//!
//! Executes built-in tools (moves) with capability checking, timeout
//! enforcement, and SSRF protection for network-facing tools.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use dashmap::DashMap;
use tokio::process::Command;
use tracing::{debug, instrument, warn};

use punch_extensions::plugin::PluginRegistry;
use punch_memory::MemorySubstrate;
use punch_types::{
    AgentCoordinator, ApprovalDecision, BrowserPool, Capability, ChannelNotifier, FighterId,
    PolicyEngine, PunchError, PunchResult, SandboxEnforcer, Sensitivity, ShellBleedDetector,
    ToolResult, capability::capability_matches,
};

use crate::mcp::McpClient;

/// Context passed to every tool execution.
pub struct ToolExecutionContext {
    /// Working directory for filesystem and shell operations.
    pub working_dir: PathBuf,
    /// The fighter invoking the tool.
    pub fighter_id: FighterId,
    /// Memory substrate for memory/knowledge tools.
    pub memory: Arc<MemorySubstrate>,
    /// Optional agent coordinator for inter-agent tools (agent_spawn, agent_message, agent_list).
    /// This is `None` when the fighter does not have agent coordination capabilities.
    pub coordinator: Option<Arc<dyn AgentCoordinator>>,
    /// Optional policy engine for approval-gated tool execution.
    /// When present, every tool call is checked against the configured policies
    /// before dispatching. The referee must approve the move.
    pub approval_engine: Option<Arc<PolicyEngine>>,
    /// Optional subprocess sandbox (containment ring) for shell and filesystem tools.
    /// When present, commands are validated and environments are sanitized before execution.
    pub sandbox: Option<Arc<SandboxEnforcer>>,
    /// Optional shell bleed detector — scans shell commands for leaked secrets
    /// before the move lands. If a Secret or Confidential bleed is detected,
    /// the command is blocked.
    pub bleed_detector: Option<Arc<ShellBleedDetector>>,
    /// Optional browser session pool for browser automation tools.
    /// When present, browser scouting moves (navigate, screenshot, click, etc.)
    /// can manage sessions through the pool. The actual CDP driver is plugged in
    /// separately — without it, browser tools report "browser not available".
    pub browser_pool: Option<Arc<BrowserPool>>,
    /// Optional plugin registry for WASM plugin invocation.
    /// When present, the `wasm_invoke` tool can dispatch calls to loaded
    /// WASM plugins (imported techniques). Without it, the tool reports
    /// "plugin runtime not configured".
    pub plugin_registry: Option<Arc<PluginRegistry>>,
    /// Active MCP server clients, keyed by server name.
    /// When present, tools prefixed with `mcp_{server}_` are routed to the
    /// corresponding MCP server for execution.
    pub mcp_clients: Option<Arc<DashMap<String, Arc<McpClient>>>>,
    /// Optional channel notifier for proactive outbound messaging.
    /// When present, the `channel_notify` tool can send messages to
    /// connected channels (Telegram, Slack, Discord, etc.).
    pub channel_notifier: Option<Arc<dyn ChannelNotifier>>,
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
    // --- Approval gate: check the referee before throwing the move ---
    if let Some(ref engine) = context.approval_engine {
        let decision = engine.evaluate(name, input, &context.fighter_id).await?;
        match decision {
            ApprovalDecision::Allow => {
                // Move approved — proceed to dispatch.
            }
            ApprovalDecision::Deny(reason) => {
                debug!(tool = %name, reason = %reason, "tool call denied by approval policy");
                return Ok(ToolResult {
                    success: false,
                    output: serde_json::json!(null),
                    error: Some(format!("denied by policy: {}", reason)),
                    duration_ms: 0,
                });
            }
            ApprovalDecision::NeedsApproval(reason) => {
                debug!(tool = %name, reason = %reason, "tool call needs approval");
                return Ok(ToolResult {
                    success: false,
                    output: serde_json::json!(null),
                    error: Some(format!("approval required: {}", reason)),
                    duration_ms: 0,
                });
            }
        }
    }

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
        "agent_spawn" => tool_agent_spawn(input, capabilities, context).await,
        "agent_message" => tool_agent_message(input, capabilities, context).await,
        "agent_list" => tool_agent_list(capabilities, context).await,
        "patch_apply" => tool_patch_apply(input, capabilities, context).await,
        "browser_navigate" => tool_browser_navigate(input, capabilities, context).await,
        "browser_screenshot" => tool_browser_screenshot(input, capabilities, context).await,
        "browser_click" => tool_browser_click(input, capabilities, context).await,
        "browser_type" => tool_browser_type(input, capabilities, context).await,
        "browser_content" => tool_browser_content(input, capabilities, context).await,
        // Git / Source Control
        "git_status" => tool_git_status(input, capabilities, context).await,
        "git_diff" => tool_git_diff(input, capabilities, context).await,
        "git_log" => tool_git_log(input, capabilities, context).await,
        "git_commit" => tool_git_commit(input, capabilities, context).await,
        "git_branch" => tool_git_branch(input, capabilities, context).await,
        // Container
        "docker_ps" => tool_docker_ps(input, capabilities).await,
        "docker_run" => tool_docker_run(input, capabilities).await,
        "docker_build" => tool_docker_build(input, capabilities, context).await,
        "docker_logs" => tool_docker_logs(input, capabilities).await,
        // HTTP
        "http_request" => tool_http_request(input, capabilities).await,
        "http_post" => tool_http_post(input, capabilities).await,
        // Data manipulation
        "json_query" => tool_json_query(input, capabilities).await,
        "json_transform" => tool_json_transform(input, capabilities).await,
        "yaml_parse" => tool_yaml_parse(input, capabilities).await,
        "regex_match" => tool_regex_match(input, capabilities).await,
        "regex_replace" => tool_regex_replace(input, capabilities).await,
        // Process
        "process_list" => tool_process_list(input, capabilities, context).await,
        "process_kill" => tool_process_kill(input, capabilities).await,
        // Schedule
        "schedule_task" => tool_schedule_task(input, capabilities, context).await,
        "schedule_list" => tool_schedule_list(capabilities).await,
        "schedule_cancel" => tool_schedule_cancel(input, capabilities).await,
        // Code analysis
        "code_search" => tool_code_search(input, capabilities, context).await,
        "code_symbols" => tool_code_symbols(input, capabilities, context).await,
        // Archive
        "archive_create" => tool_archive_create(input, capabilities, context).await,
        "archive_extract" => tool_archive_extract(input, capabilities, context).await,
        "archive_list" => tool_archive_list(input, capabilities, context).await,
        // Template
        "template_render" => tool_template_render(input, capabilities).await,
        // Crypto / Hash
        "hash_compute" => tool_hash_compute(input, capabilities, context).await,
        "hash_verify" => tool_hash_verify(input, capabilities, context).await,
        // Environment
        "env_get" => tool_env_get(input, capabilities).await,
        "env_list" => tool_env_list(input, capabilities).await,
        // Text
        "text_diff" => tool_text_diff(input, capabilities).await,
        "text_count" => tool_text_count(input, capabilities).await,
        // File (extended)
        "file_search" => tool_file_search(input, capabilities, context).await,
        "file_info" => tool_file_info(input, capabilities, context).await,
        // WASM Plugin
        "wasm_invoke" => tool_wasm_invoke(input, capabilities, context).await,
        // A2A delegation
        "a2a_delegate" => tool_a2a_delegate(input, capabilities).await,
        // Channel notification
        "channel_notify" => tool_channel_notify(input, capabilities, context).await,
        // MCP server tools — dispatched by `mcp_{server}_{tool}` prefix.
        _ if name.starts_with("mcp_") => tool_mcp_call(name, input, capabilities, context).await,
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
// Sensitive path detection
// ---------------------------------------------------------------------------

/// Paths that are considered sensitive and should be flagged by the bleed
/// detector when accessed. These are common locations for secrets, credentials,
/// and private keys.
static SENSITIVE_PATH_PATTERNS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        ".env",
        ".ssh/",
        ".gnupg/",
        ".aws/credentials",
        ".aws/config",
        ".npmrc",
        ".pypirc",
        ".docker/config.json",
        ".kube/config",
        ".netrc",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        "credentials.json",
        "service_account.json",
        "secrets.yaml",
        "secrets.yml",
        "secrets.json",
        "/etc/shadow",
        "/etc/passwd",
    ]
});

/// Check whether a path matches any known sensitive pattern.
fn is_sensitive_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    SENSITIVE_PATH_PATTERNS
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

// ---------------------------------------------------------------------------
// Channel notification
// ---------------------------------------------------------------------------

/// Send a proactive message to an external channel (Telegram, Slack, Discord, etc.).
async fn tool_channel_notify(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::ChannelNotify)?;

    let notifier = context
        .channel_notifier
        .as_ref()
        .ok_or_else(|| PunchError::Tool {
            tool: "channel_notify".into(),
            message: "channel notifier not configured — no channel adapters are available".into(),
        })?;

    let channel = input["channel"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "channel_notify".into(),
        message: "missing 'channel' parameter (e.g., \"telegram\", \"discord\", \"slack\")".into(),
    })?;

    let chat_id = input["chat_id"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "channel_notify".into(),
        message: "missing 'chat_id' parameter (the channel/conversation ID to send to)".into(),
    })?;

    let message = input["message"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "channel_notify".into(),
        message: "missing 'message' parameter (the text to send)".into(),
    })?;

    debug!(
        channel = %channel,
        chat_id = %chat_id,
        message_len = message.len(),
        "channel_notify: sending proactive message"
    );

    notifier.notify(channel, chat_id, message).await?;

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "sent": true,
            "channel": channel,
            "chat_id": chat_id,
            "message_length": message.len(),
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// MCP tool dispatch
// ---------------------------------------------------------------------------

/// Route a tool call to the appropriate MCP server.
///
/// Tool names follow the convention `mcp_{server}_{tool}`. This function
/// finds the matching server, checks the `McpAccess` capability, strips the
/// namespace prefix, and forwards the call.
async fn tool_mcp_call(
    name: &str,
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let clients = context.mcp_clients.as_ref().ok_or_else(|| {
        PunchError::ToolNotFound(format!(
            "MCP tool '{}' requested but no MCP servers are configured",
            name
        ))
    })?;

    // Find the matching MCP server by trying each client's strip_namespace.
    let mut matched_client: Option<Arc<McpClient>> = None;
    let mut raw_tool_name: Option<String> = None;

    for entry in clients.iter() {
        if let Some(stripped) = entry.value().strip_namespace(name) {
            // Check capability: the fighter must have McpAccess for this server.
            require_capability(capabilities, &Capability::McpAccess(entry.key().clone()))?;
            matched_client = Some(Arc::clone(entry.value()));
            raw_tool_name = Some(stripped.to_string());
            break;
        }
    }

    let client = matched_client.ok_or_else(|| {
        PunchError::ToolNotFound(format!("no MCP server matches tool '{}'", name))
    })?;
    let raw_name = raw_tool_name.unwrap();

    debug!(
        server = %client.server_name(),
        tool = %raw_name,
        "dispatching MCP tool call"
    );

    match client.call_tool(&raw_name, input.clone()).await {
        Ok(result) => {
            // Extract text content from MCP response format.
            let output = if let Some(content) = result.get("content") {
                if let Some(arr) = content.as_array() {
                    arr.iter()
                        .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                }
            } else {
                serde_json::to_string_pretty(&result).unwrap_or_default()
            };

            let is_error = result
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            Ok(ToolResult {
                success: !is_error,
                output: serde_json::Value::String(output),
                error: if is_error {
                    Some("MCP tool returned error".to_string())
                } else {
                    None
                },
                duration_ms: 0,
            })
        }
        Err(e) => Ok(ToolResult {
            success: false,
            output: serde_json::json!(null),
            error: Some(format!("MCP call failed: {}", e)),
            duration_ms: 0,
        }),
    }
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

    // If a sandbox is active, validate the path through the containment ring.
    if let Some(ref sandbox) = context.sandbox {
        sandbox.validate_path(&path).map_err(|v| PunchError::Tool {
            tool: "file_read".into(),
            message: v.to_string(),
        })?;
    }

    // Sensitive path detection: flag reads of known secret/credential locations.
    if is_sensitive_path(&path_display) {
        warn!(
            path = %path_display,
            fighter = %context.fighter_id,
            "sensitive path access detected during file_read"
        );

        // If a bleed detector is active, block reads of sensitive paths.
        if context.bleed_detector.is_some() {
            return Ok(ToolResult {
                success: false,
                output: serde_json::json!(null),
                error: Some(format!(
                    "security: read of sensitive path '{}' blocked by bleed detector",
                    path_display
                )),
                duration_ms: 0,
            });
        }
    }

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            // Scan file content for leaked secrets if bleed detector is active.
            if let Some(ref detector) = context.bleed_detector {
                let warnings = detector.scan_command(&content);
                let secret_warnings: Vec<_> = warnings
                    .iter()
                    .filter(|w| w.severity >= Sensitivity::Confidential)
                    .collect();
                if !secret_warnings.is_empty() {
                    warn!(
                        path = %path_display,
                        warning_count = secret_warnings.len(),
                        "file content contains potential secrets"
                    );
                }
            }

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

    // If a sandbox is active, validate the path through the containment ring.
    if let Some(ref sandbox) = context.sandbox {
        sandbox.validate_path(&path).map_err(|v| PunchError::Tool {
            tool: "file_write".into(),
            message: v.to_string(),
        })?;
    }

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

/// Apply a unified diff patch to a file — execute a combo correction move.
///
/// Reads the target file, parses the diff, validates it against the current
/// content, applies the patch, and writes the result back.
async fn tool_patch_apply(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let path_str = input["path"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "patch_apply".into(),
        message: "missing 'path' parameter".into(),
    })?;
    let diff_text = input["diff"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "patch_apply".into(),
        message: "missing 'diff' parameter".into(),
    })?;

    let path = resolve_path(&context.working_dir, path_str)?;
    let path_display = path.display().to_string();

    // Patch application requires file write capability.
    require_capability(capabilities, &Capability::FileWrite(path_display.clone()))?;

    // Validate path through sandbox if active.
    if let Some(ref sandbox) = context.sandbox {
        sandbox.validate_path(&path).map_err(|v| PunchError::Tool {
            tool: "patch_apply".into(),
            message: v.to_string(),
        })?;
    }

    // Parse the diff.
    let patch_set = punch_types::parse_unified_diff(diff_text).map_err(|e| PunchError::Tool {
        tool: "patch_apply".into(),
        message: format!("failed to parse diff: {}", e),
    })?;

    if patch_set.patches.is_empty() {
        return Ok(ToolResult {
            success: false,
            output: serde_json::json!(null),
            error: Some("diff contains no file patches".into()),
            duration_ms: 0,
        });
    }

    // Use the first patch in the set (the tool operates on a single file).
    let file_patch = &patch_set.patches[0];

    // Read the current file content (empty string for new files).
    let original = if file_patch.is_new_file {
        String::new()
    } else {
        tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| PunchError::Tool {
                tool: "patch_apply".into(),
                message: format!("failed to read '{}': {}", path_display, e),
            })?
    };

    // Validate before applying.
    let conflicts = punch_types::validate_patch(&original, file_patch);
    if !conflicts.is_empty() {
        let conflict_desc: Vec<String> = conflicts
            .iter()
            .map(|c| {
                format!(
                    "hunk {}: line {} — expected {:?}, found {:?} ({:?})",
                    c.hunk_index + 1,
                    c.line_number,
                    c.expected_line,
                    c.actual_line,
                    c.conflict_type
                )
            })
            .collect();

        // Try fuzzy application with a small fuzz factor.
        match punch_types::apply_patch_fuzzy(&original, file_patch, 3) {
            Ok(patched) => {
                // Fuzzy succeeded — write with a warning.
                if let Some(parent) = path.parent()
                    && !parent.exists()
                {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| PunchError::Tool {
                            tool: "patch_apply".into(),
                            message: format!(
                                "failed to create directory '{}': {}",
                                parent.display(),
                                e
                            ),
                        })?;
                }
                tokio::fs::write(&path, &patched)
                    .await
                    .map_err(|e| PunchError::Tool {
                        tool: "patch_apply".into(),
                        message: format!("failed to write '{}': {}", path_display, e),
                    })?;
                debug!(path = %path_display, "patch applied with fuzzy matching");
                return Ok(ToolResult {
                    success: true,
                    output: serde_json::json!(format!(
                        "patch applied to {} with fuzzy matching (offset adjustments needed). Warnings: {}",
                        path_display,
                        conflict_desc.join("; ")
                    )),
                    error: None,
                    duration_ms: 0,
                });
            }
            Err(_) => {
                return Ok(ToolResult {
                    success: false,
                    output: serde_json::json!(null),
                    error: Some(format!(
                        "patch conflicts detected: {}",
                        conflict_desc.join("; ")
                    )),
                    duration_ms: 0,
                });
            }
        }
    }

    // Clean application.
    let patched =
        punch_types::apply_patch(&original, file_patch).map_err(|e| PunchError::Tool {
            tool: "patch_apply".into(),
            message: format!("failed to apply patch: {}", e),
        })?;

    // Ensure parent directory exists.
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| PunchError::Tool {
                tool: "patch_apply".into(),
                message: format!("failed to create directory '{}': {}", parent.display(), e),
            })?;
    }

    tokio::fs::write(&path, &patched)
        .await
        .map_err(|e| PunchError::Tool {
            tool: "patch_apply".into(),
            message: format!("failed to write '{}': {}", path_display, e),
        })?;

    debug!(path = %path_display, "patch applied cleanly");
    Ok(ToolResult {
        success: true,
        output: serde_json::json!(format!("patch applied cleanly to {}", path_display)),
        error: None,
        duration_ms: 0,
    })
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

    // Shell bleed detection: scan the command for leaked secrets before the
    // punch lands. Secret and Confidential bleeds block the move outright.
    if let Some(ref detector) = context.bleed_detector {
        let warnings = detector.scan_command(command_str);
        let blocked: Vec<_> = warnings
            .iter()
            .filter(|w| w.severity >= Sensitivity::Confidential)
            .collect();

        if !blocked.is_empty() {
            let details: Vec<String> = blocked
                .iter()
                .map(|w| {
                    format!(
                        "[{}] {} (severity: {})",
                        w.pattern_name, w.location, w.severity
                    )
                })
                .collect();
            return Ok(ToolResult {
                success: false,
                output: serde_json::json!(null),
                error: Some(format!(
                    "shell bleed detected — command blocked: {}",
                    details.join("; ")
                )),
                duration_ms: 0,
            });
        }

        // Internal-severity warnings: log but allow execution.
        for w in &warnings {
            if w.severity == Sensitivity::Internal {
                tracing::warn!(
                    pattern = %w.pattern_name,
                    location = %w.location,
                    "shell bleed warning (internal severity) — allowing execution"
                );
            }
        }
    }

    // Note: Shell execution is capability-gated. The command string comes from
    // the LLM and is validated via the ShellExec capability pattern before
    // execution. This is intentional for an agent runtime that needs to run
    // arbitrary commands on behalf of the user.
    //
    // If a sandbox is active, the command enters the containment ring:
    // validated, environment-sanitized, and directory-restricted.
    let output = if let Some(ref sandbox) = context.sandbox {
        let mut cmd = sandbox
            .build_command(command_str)
            .map_err(|v| PunchError::Tool {
                tool: "shell_exec".into(),
                message: v.to_string(),
            })?;
        cmd.current_dir(&context.working_dir);
        cmd.output().await.map_err(|e| PunchError::Tool {
            tool: "shell_exec".into(),
            message: format!("failed to execute command: {}", e),
        })?
    } else {
        Command::new("sh")
            .arg("-c")
            .arg(command_str)
            .current_dir(&context.working_dir)
            .output()
            .await
            .map_err(|e| PunchError::Tool {
                tool: "shell_exec".into(),
                message: format!("failed to execute command: {}", e),
            })?
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    debug!(exit_code = exit_code, "shell exec complete");

    // Post-execution output scanning: check stdout and stderr for leaked secrets.
    if let Some(ref detector) = context.bleed_detector {
        let stdout_warnings = detector.scan_command(&stdout);
        let stderr_warnings = detector.scan_command(&stderr);

        let all_warnings: Vec<_> = stdout_warnings
            .iter()
            .chain(stderr_warnings.iter())
            .filter(|w| w.severity >= Sensitivity::Confidential)
            .collect();

        if !all_warnings.is_empty() {
            let details: Vec<String> = all_warnings
                .iter()
                .map(|w| {
                    format!(
                        "[{}] {} (severity: {})",
                        w.pattern_name, w.location, w.severity
                    )
                })
                .collect();
            warn!(
                warning_count = all_warnings.len(),
                details = %details.join("; "),
                "shell output contains potential secrets — flagging security event"
            );
        }
    }

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

async fn tool_web_search(input: &serde_json::Value) -> PunchResult<ToolResult> {
    let query = input["query"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "web_search".into(),
        message: "missing 'query' parameter".into(),
    })?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| PunchError::Tool {
            tool: "web_search".into(),
            message: format!("failed to create HTTP client: {}", e),
        })?;

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );

    let response = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (compatible; PunchAgent/1.0)")
        .send()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "web_search".into(),
            message: format!("search request failed: {}", e),
        })?;

    let body = response.text().await.map_err(|e| PunchError::Tool {
        tool: "web_search".into(),
        message: format!("failed to read search response: {}", e),
    })?;

    let results = parse_duckduckgo_results(&body);

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(results),
        error: None,
        duration_ms: 0,
    })
}

/// Parse DuckDuckGo HTML search results to extract titles and URLs.
fn parse_duckduckgo_results(html: &str) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    let mut remaining = html;

    // DuckDuckGo HTML results contain links with class "result__a".
    // We parse them with simple string scanning rather than pulling in
    // a full HTML parser dependency.
    while results.len() < 5 {
        // Look for result links: <a rel="nofollow" class="result__a" href="..."
        let marker = "class=\"result__a\"";
        let Some(pos) = remaining.find(marker) else {
            break;
        };
        remaining = &remaining[pos + marker.len()..];

        // Extract href.
        let href = if let Some(href_pos) = remaining.find("href=\"") {
            let start = href_pos + 6;
            let href_rest = &remaining[start..];
            if let Some(end) = href_rest.find('"') {
                let raw_href = &href_rest[..end];
                // DuckDuckGo wraps URLs in a redirect; extract the actual URL.
                if let Some(uddg_pos) = raw_href.find("uddg=") {
                    let encoded = &raw_href[uddg_pos + 5..];
                    let decoded = urlencoding::decode(encoded)
                        .unwrap_or_else(|_| encoded.into())
                        .to_string();
                    // Strip any trailing &rut= parameter.
                    decoded.split('&').next().unwrap_or(&decoded).to_string()
                } else {
                    raw_href.to_string()
                }
            } else {
                continue;
            }
        } else {
            continue;
        };

        // Extract title text (content between > and </a>).
        let title = if let Some(gt_pos) = remaining.find('>') {
            let after_gt = &remaining[gt_pos + 1..];
            if let Some(end_tag) = after_gt.find("</a>") {
                let raw_title = &after_gt[..end_tag];
                // Strip any HTML tags from the title text.
                strip_html_tags(raw_title).trim().to_string()
            } else {
                "Untitled".to_string()
            }
        } else {
            "Untitled".to_string()
        };

        if !title.is_empty() && !href.is_empty() {
            results.push(serde_json::json!({
                "title": title,
                "url": href,
            }));
        }
    }

    results
}

/// Strip HTML tags from a string, returning only text content.
fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
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
// Agent coordination tools
// ---------------------------------------------------------------------------

/// Helper to get the coordinator or return an error.
fn get_coordinator(context: &ToolExecutionContext) -> PunchResult<&dyn AgentCoordinator> {
    context
        .coordinator
        .as_deref()
        .ok_or_else(|| PunchError::Tool {
            tool: "agent".into(),
            message: "agent coordinator not available in this context".into(),
        })
}

async fn tool_agent_spawn(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::AgentSpawn)?;

    let coordinator = get_coordinator(context)?;

    let name = input["name"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "agent_spawn".into(),
        message: "missing 'name' parameter".into(),
    })?;

    let system_prompt = input["system_prompt"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "agent_spawn".into(),
            message: "missing 'system_prompt' parameter".into(),
        })?;

    let description = input["description"]
        .as_str()
        .unwrap_or("Spawned by another agent");

    // Build a manifest for the new fighter. We use sensible defaults
    // and let the coordinator (Ring) handle persistence and model config.
    use punch_types::{FighterManifest, ModelConfig, Provider, WeightClass};

    // Parse capabilities for the child agent if provided.
    let child_capabilities: Vec<punch_types::Capability> =
        if let Some(caps) = input.get("capabilities") {
            serde_json::from_value(caps.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };

    let manifest = FighterManifest {
        name: name.to_string(),
        description: description.to_string(),
        model: ModelConfig {
            provider: Provider::Ollama,
            model: "gpt-oss:20b".to_string(),
            api_key_env: None,
            base_url: Some("http://localhost:11434".to_string()),
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
        system_prompt: system_prompt.to_string(),
        capabilities: child_capabilities,
        weight_class: WeightClass::Featherweight,
        tenant_id: None,
    };

    let fighter_id = coordinator.spawn_fighter(manifest).await?;

    debug!(fighter_id = %fighter_id, name = %name, "agent_spawn: fighter spawned");

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "fighter_id": fighter_id.0.to_string(),
            "name": name,
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_agent_message(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::AgentMessage)?;

    let coordinator = get_coordinator(context)?;

    // Accept either "fighter_id" or "name" to identify the target.
    let target_id = if let Some(id_str) = input["fighter_id"].as_str() {
        let uuid = uuid::Uuid::parse_str(id_str).map_err(|e| PunchError::Tool {
            tool: "agent_message".into(),
            message: format!("invalid fighter_id '{}': {}", id_str, e),
        })?;
        punch_types::FighterId(uuid)
    } else if let Some(name) = input["name"].as_str() {
        coordinator
            .find_fighter_by_name(name)
            .await?
            .ok_or_else(|| PunchError::Tool {
                tool: "agent_message".into(),
                message: format!("no fighter found with name '{}'", name),
            })?
    } else {
        return Err(PunchError::Tool {
            tool: "agent_message".into(),
            message: "must provide either 'fighter_id' or 'name' parameter".into(),
        });
    };

    let message = input["message"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "agent_message".into(),
            message: "missing 'message' parameter".into(),
        })?
        .to_string();

    debug!(
        target = %target_id,
        from = %context.fighter_id,
        "agent_message: sending inter-agent message"
    );

    let result = coordinator
        .send_message_to_agent(&target_id, message)
        .await?;

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "response": result.response,
            "tokens_used": result.tokens_used,
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_agent_list(
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::AgentMessage)?;

    let coordinator = get_coordinator(context)?;

    let agents = coordinator.list_fighters().await?;

    let agent_list: Vec<serde_json::Value> = agents
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id.0.to_string(),
                "name": a.name,
                "status": format!("{}", a.status),
            })
        })
        .collect();

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(agent_list),
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
// Browser automation tool handlers — ring-side scouting moves
// ---------------------------------------------------------------------------

/// Helper: check BrowserControl capability and verify the browser pool is available.
fn require_browser_pool<'a>(
    capabilities: &[Capability],
    context: &'a ToolExecutionContext,
) -> PunchResult<&'a Arc<BrowserPool>> {
    require_capability(capabilities, &Capability::BrowserControl)?;
    context
        .browser_pool
        .as_ref()
        .ok_or_else(|| PunchError::Tool {
            tool: "browser".into(),
            message: "browser not available — no CDP driver configured".into(),
        })
}

async fn tool_browser_navigate(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let _pool = require_browser_pool(capabilities, context)?;

    let url = input["url"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "browser_navigate".into(),
        message: "missing 'url' parameter".into(),
    })?;

    debug!(url = %url, "browser_navigate requested (no CDP driver)");

    Ok(ToolResult {
        success: false,
        output: serde_json::json!({
            "action": "navigate",
            "url": url,
            "message": "browser pool is available but no CDP driver is configured — install a BrowserDriver to enable navigation"
        }),
        error: Some("no CDP driver configured".into()),
        duration_ms: 0,
    })
}

async fn tool_browser_screenshot(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let _pool = require_browser_pool(capabilities, context)?;

    let full_page = input["full_page"].as_bool().unwrap_or(false);

    debug!(full_page = %full_page, "browser_screenshot requested (no CDP driver)");

    Ok(ToolResult {
        success: false,
        output: serde_json::json!({
            "action": "screenshot",
            "full_page": full_page,
            "message": "browser pool is available but no CDP driver is configured — install a BrowserDriver to enable screenshots"
        }),
        error: Some("no CDP driver configured".into()),
        duration_ms: 0,
    })
}

async fn tool_browser_click(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let _pool = require_browser_pool(capabilities, context)?;

    let selector = input["selector"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "browser_click".into(),
        message: "missing 'selector' parameter".into(),
    })?;

    debug!(selector = %selector, "browser_click requested (no CDP driver)");

    Ok(ToolResult {
        success: false,
        output: serde_json::json!({
            "action": "click",
            "selector": selector,
            "message": "browser pool is available but no CDP driver is configured — install a BrowserDriver to enable clicking"
        }),
        error: Some("no CDP driver configured".into()),
        duration_ms: 0,
    })
}

async fn tool_browser_type(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let _pool = require_browser_pool(capabilities, context)?;

    let selector = input["selector"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "browser_type".into(),
        message: "missing 'selector' parameter".into(),
    })?;
    let text = input["text"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "browser_type".into(),
        message: "missing 'text' parameter".into(),
    })?;

    debug!(selector = %selector, text_len = text.len(), "browser_type requested (no CDP driver)");

    Ok(ToolResult {
        success: false,
        output: serde_json::json!({
            "action": "type",
            "selector": selector,
            "text_length": text.len(),
            "message": "browser pool is available but no CDP driver is configured — install a BrowserDriver to enable typing"
        }),
        error: Some("no CDP driver configured".into()),
        duration_ms: 0,
    })
}

async fn tool_browser_content(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let _pool = require_browser_pool(capabilities, context)?;

    let selector = input["selector"].as_str();

    debug!(selector = ?selector, "browser_content requested (no CDP driver)");

    Ok(ToolResult {
        success: false,
        output: serde_json::json!({
            "action": "get_content",
            "selector": selector,
            "message": "browser pool is available but no CDP driver is configured — install a BrowserDriver to enable content extraction"
        }),
        error: Some("no CDP driver configured".into()),
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Git / Source Control tool implementations
// ---------------------------------------------------------------------------

async fn tool_git_status(
    _input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::SourceControl)?;

    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&context.working_dir)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "git_status".into(),
            message: format!("failed to run git status: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!({
            "status": stdout,
            "stderr": stderr,
        }),
        error: if output.status.success() {
            None
        } else {
            Some(stderr)
        },
        duration_ms: 0,
    })
}

async fn tool_git_diff(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::SourceControl)?;

    let staged = input["staged"].as_bool().unwrap_or(false);
    let mut args = vec!["diff".to_string()];
    if staged {
        args.push("--staged".to_string());
    }
    if let Some(path) = input["path"].as_str() {
        args.push("--".to_string());
        args.push(path.to_string());
    }

    let output = Command::new("git")
        .args(&args)
        .current_dir(&context.working_dir)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "git_diff".into(),
            message: format!("failed to run git diff: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!(stdout),
        error: if output.status.success() {
            None
        } else {
            Some(stderr)
        },
        duration_ms: 0,
    })
}

async fn tool_git_log(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::SourceControl)?;

    let count = input["count"].as_u64().unwrap_or(10);
    let output = Command::new("git")
        .args(["log", "--oneline", "-n", &count.to_string()])
        .current_dir(&context.working_dir)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "git_log".into(),
            message: format!("failed to run git log: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!(stdout),
        error: if output.status.success() {
            None
        } else {
            Some(stderr)
        },
        duration_ms: 0,
    })
}

async fn tool_git_commit(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::SourceControl)?;

    let message = input["message"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "git_commit".into(),
        message: "missing 'message' parameter".into(),
    })?;

    // Stage files if specified.
    if let Some(files) = input["files"].as_array() {
        let file_args: Vec<&str> = files.iter().filter_map(|f| f.as_str()).collect();
        if !file_args.is_empty() {
            let mut add_args = vec!["add"];
            add_args.extend(file_args);
            let add_output = Command::new("git")
                .args(&add_args)
                .current_dir(&context.working_dir)
                .output()
                .await
                .map_err(|e| PunchError::Tool {
                    tool: "git_commit".into(),
                    message: format!("failed to stage files: {}", e),
                })?;

            if !add_output.status.success() {
                let stderr = String::from_utf8_lossy(&add_output.stderr);
                return Ok(ToolResult {
                    success: false,
                    output: serde_json::json!(null),
                    error: Some(format!("git add failed: {}", stderr)),
                    duration_ms: 0,
                });
            }
        }
    }

    let output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(&context.working_dir)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "git_commit".into(),
            message: format!("failed to run git commit: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!(stdout),
        error: if output.status.success() {
            None
        } else {
            Some(stderr)
        },
        duration_ms: 0,
    })
}

async fn tool_git_branch(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::SourceControl)?;

    let action = input["action"].as_str().unwrap_or("list");

    let output = match action {
        "list" => {
            Command::new("git")
                .args(["branch", "--list"])
                .current_dir(&context.working_dir)
                .output()
                .await
        }
        "create" => {
            let name = input["name"].as_str().ok_or_else(|| PunchError::Tool {
                tool: "git_branch".into(),
                message: "missing 'name' parameter for create".into(),
            })?;
            Command::new("git")
                .args(["branch", name])
                .current_dir(&context.working_dir)
                .output()
                .await
        }
        "switch" => {
            let name = input["name"].as_str().ok_or_else(|| PunchError::Tool {
                tool: "git_branch".into(),
                message: "missing 'name' parameter for switch".into(),
            })?;
            Command::new("git")
                .args(["checkout", name])
                .current_dir(&context.working_dir)
                .output()
                .await
        }
        other => {
            return Ok(ToolResult {
                success: false,
                output: serde_json::json!(null),
                error: Some(format!(
                    "unknown action '{}', expected list/create/switch",
                    other
                )),
                duration_ms: 0,
            });
        }
    }
    .map_err(|e| PunchError::Tool {
        tool: "git_branch".into(),
        message: format!("failed to run git branch: {}", e),
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!(stdout),
        error: if output.status.success() {
            None
        } else {
            Some(stderr)
        },
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Container tool implementations
// ---------------------------------------------------------------------------

async fn tool_docker_ps(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Container)?;

    let show_all = input["all"].as_bool().unwrap_or(false);
    let mut args = vec![
        "ps",
        "--format",
        "{{.ID}}\t{{.Image}}\t{{.Status}}\t{{.Names}}",
    ];
    if show_all {
        args.push("-a");
    }

    let output = Command::new("docker")
        .args(&args)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "docker_ps".into(),
            message: format!("failed to run docker ps: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let containers: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(4, '\t').collect();
            serde_json::json!({
                "id": parts.first().unwrap_or(&""),
                "image": parts.get(1).unwrap_or(&""),
                "status": parts.get(2).unwrap_or(&""),
                "name": parts.get(3).unwrap_or(&""),
            })
        })
        .collect();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!(containers),
        error: if output.status.success() {
            None
        } else {
            Some(stderr)
        },
        duration_ms: 0,
    })
}

async fn tool_docker_run(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Container)?;

    let image = input["image"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "docker_run".into(),
        message: "missing 'image' parameter".into(),
    })?;

    let detach = input["detach"].as_bool().unwrap_or(false);
    let mut args = vec!["run".to_string()];

    if detach {
        args.push("-d".to_string());
    }

    if let Some(name) = input["name"].as_str() {
        args.push("--name".to_string());
        args.push(name.to_string());
    }

    if let Some(env) = input["env"].as_object() {
        for (key, val) in env {
            args.push("-e".to_string());
            args.push(format!("{}={}", key, val.as_str().unwrap_or_default()));
        }
    }

    if let Some(ports) = input["ports"].as_array() {
        for port in ports {
            if let Some(p) = port.as_str() {
                args.push("-p".to_string());
                args.push(p.to_string());
            }
        }
    }

    args.push(image.to_string());

    if let Some(cmd) = input["command"].as_str() {
        // Split the command string on whitespace for the container command.
        for part in cmd.split_whitespace() {
            args.push(part.to_string());
        }
    }

    let output = Command::new("docker")
        .args(&args)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "docker_run".into(),
            message: format!("failed to run docker run: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!({
            "stdout": stdout.trim(),
            "stderr": stderr.trim(),
        }),
        error: if output.status.success() {
            None
        } else {
            Some(stderr)
        },
        duration_ms: 0,
    })
}

async fn tool_docker_build(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Container)?;

    let build_path = input["path"].as_str().unwrap_or(".");
    let resolved_path = resolve_path(&context.working_dir, build_path)?;

    let mut args = vec!["build".to_string()];

    if let Some(tag) = input["tag"].as_str() {
        args.push("-t".to_string());
        args.push(tag.to_string());
    }

    if let Some(dockerfile) = input["dockerfile"].as_str() {
        args.push("-f".to_string());
        args.push(dockerfile.to_string());
    }

    args.push(resolved_path.display().to_string());

    let output = Command::new("docker")
        .args(&args)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "docker_build".into(),
            message: format!("failed to run docker build: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Truncate long build output.
    let truncated_stdout = if stdout.len() > 10_000 {
        format!("{}... [truncated]", &stdout[..10_000])
    } else {
        stdout
    };

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!({
            "stdout": truncated_stdout,
            "stderr": stderr,
        }),
        error: if output.status.success() {
            None
        } else {
            Some(stderr)
        },
        duration_ms: 0,
    })
}

async fn tool_docker_logs(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Container)?;

    let container = input["container"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "docker_logs".into(),
            message: "missing 'container' parameter".into(),
        })?;

    let tail = input["tail"].as_u64().unwrap_or(100);

    let output = Command::new("docker")
        .args(["logs", "--tail", &tail.to_string(), container])
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "docker_logs".into(),
            message: format!("failed to run docker logs: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!({
            "logs": format!("{}{}", stdout, stderr),
        }),
        error: if output.status.success() {
            None
        } else {
            Some(format!("docker logs failed: {}", stderr))
        },
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// HTTP tool implementations
// ---------------------------------------------------------------------------

async fn tool_http_request(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    let url_str = input["url"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "http_request".into(),
        message: "missing 'url' parameter".into(),
    })?;

    let parsed_url = url::Url::parse(url_str).map_err(|e| PunchError::Tool {
        tool: "http_request".into(),
        message: format!("invalid URL: {}", e),
    })?;

    if let Some(host) = parsed_url.host_str() {
        require_capability(capabilities, &Capability::Network(host.to_string()))?;
    }

    let method_str = input["method"].as_str().unwrap_or("GET");
    let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(30);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| PunchError::Tool {
            tool: "http_request".into(),
            message: format!("failed to create HTTP client: {}", e),
        })?;

    let method = method_str
        .parse::<reqwest::Method>()
        .map_err(|e| PunchError::Tool {
            tool: "http_request".into(),
            message: format!("invalid HTTP method '{}': {}", method_str, e),
        })?;

    let mut req = client.request(method, url_str);

    if let Some(headers) = input["headers"].as_object() {
        for (key, val) in headers {
            if let Some(v) = val.as_str() {
                req = req.header(key.as_str(), v);
            }
        }
    }

    if let Some(body) = input["body"].as_str() {
        req = req.body(body.to_string());
    }

    let response = req.send().await.map_err(|e| PunchError::Tool {
        tool: "http_request".into(),
        message: format!("request failed: {}", e),
    })?;

    let status = response.status().as_u16();
    let resp_headers: HashMap<String, String> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let body = response.text().await.map_err(|e| PunchError::Tool {
        tool: "http_request".into(),
        message: format!("failed to read response body: {}", e),
    })?;

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
            "headers": resp_headers,
            "body": truncated,
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_http_post(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    let url_str = input["url"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "http_post".into(),
        message: "missing 'url' parameter".into(),
    })?;

    let json_body = input.get("json").ok_or_else(|| PunchError::Tool {
        tool: "http_post".into(),
        message: "missing 'json' parameter".into(),
    })?;

    let parsed_url = url::Url::parse(url_str).map_err(|e| PunchError::Tool {
        tool: "http_post".into(),
        message: format!("invalid URL: {}", e),
    })?;

    if let Some(host) = parsed_url.host_str() {
        require_capability(capabilities, &Capability::Network(host.to_string()))?;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| PunchError::Tool {
            tool: "http_post".into(),
            message: format!("failed to create HTTP client: {}", e),
        })?;

    let mut req = client.post(url_str).json(json_body);

    if let Some(headers) = input["headers"].as_object() {
        for (key, val) in headers {
            if let Some(v) = val.as_str() {
                req = req.header(key.as_str(), v);
            }
        }
    }

    let response = req.send().await.map_err(|e| PunchError::Tool {
        tool: "http_post".into(),
        message: format!("request failed: {}", e),
    })?;

    let status = response.status().as_u16();
    let body = response.text().await.map_err(|e| PunchError::Tool {
        tool: "http_post".into(),
        message: format!("failed to read response body: {}", e),
    })?;

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

// ---------------------------------------------------------------------------
// Data manipulation tool implementations
// ---------------------------------------------------------------------------

/// Traverse a JSON value along a dot-separated path.
fn json_path_query(data: &serde_json::Value, path: &str) -> serde_json::Value {
    let mut current = data;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        // Try as array index first.
        if let Ok(idx) = segment.parse::<usize>()
            && let Some(val) = current.get(idx)
        {
            current = val;
            continue;
        }
        // Try as object key.
        if let Some(val) = current.get(segment) {
            current = val;
        } else {
            return serde_json::json!(null);
        }
    }
    current.clone()
}

async fn tool_json_query(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::DataManipulation)?;

    let path = input["path"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "json_query".into(),
        message: "missing 'path' parameter".into(),
    })?;

    let data = input.get("data").ok_or_else(|| PunchError::Tool {
        tool: "json_query".into(),
        message: "missing 'data' parameter".into(),
    })?;

    // If data is a string, try to parse it as JSON.
    let parsed_data = if let Some(s) = data.as_str() {
        serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!(s))
    } else {
        data.clone()
    };

    let result = json_path_query(&parsed_data, path);

    Ok(ToolResult {
        success: true,
        output: result,
        error: None,
        duration_ms: 0,
    })
}

async fn tool_json_transform(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::DataManipulation)?;

    let data = input.get("data").ok_or_else(|| PunchError::Tool {
        tool: "json_transform".into(),
        message: "missing 'data' parameter".into(),
    })?;

    // If data is a string, try to parse it as JSON.
    let mut parsed_data = if let Some(s) = data.as_str() {
        serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!(s))
    } else {
        data.clone()
    };

    // Apply key extraction.
    if let Some(extract_keys) = input["extract"].as_array() {
        let keys: Vec<&str> = extract_keys.iter().filter_map(|k| k.as_str()).collect();
        if let Some(arr) = parsed_data.as_array() {
            let filtered: Vec<serde_json::Value> = arr
                .iter()
                .map(|item| {
                    let mut obj = serde_json::Map::new();
                    for key in &keys {
                        if let Some(val) = item.get(*key) {
                            obj.insert(key.to_string(), val.clone());
                        }
                    }
                    serde_json::Value::Object(obj)
                })
                .collect();
            parsed_data = serde_json::json!(filtered);
        } else if let Some(obj) = parsed_data.as_object() {
            let mut result = serde_json::Map::new();
            for key in &keys {
                if let Some(val) = obj.get(*key) {
                    result.insert(key.to_string(), val.clone());
                }
            }
            parsed_data = serde_json::Value::Object(result);
        }
    }

    // Apply key renaming.
    if let Some(rename_map) = input["rename"].as_object() {
        if let Some(arr) = parsed_data.as_array() {
            let renamed: Vec<serde_json::Value> = arr
                .iter()
                .map(|item| {
                    if let Some(obj) = item.as_object() {
                        let mut new_obj = serde_json::Map::new();
                        for (k, v) in obj {
                            let new_key = rename_map.get(k).and_then(|r| r.as_str()).unwrap_or(k);
                            new_obj.insert(new_key.to_string(), v.clone());
                        }
                        serde_json::Value::Object(new_obj)
                    } else {
                        item.clone()
                    }
                })
                .collect();
            parsed_data = serde_json::json!(renamed);
        } else if let Some(obj) = parsed_data.as_object() {
            let mut new_obj = serde_json::Map::new();
            for (k, v) in obj {
                let new_key = rename_map.get(k).and_then(|r| r.as_str()).unwrap_or(k);
                new_obj.insert(new_key.to_string(), v.clone());
            }
            parsed_data = serde_json::Value::Object(new_obj);
        }
    }

    // Apply array filtering.
    if let Some(filter_key) = input["filter_key"].as_str()
        && let Some(filter_value) = input["filter_value"].as_str()
        && let Some(arr) = parsed_data.as_array()
    {
        let filtered: Vec<serde_json::Value> = arr
            .iter()
            .filter(|item| {
                item.get(filter_key)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s == filter_value)
            })
            .cloned()
            .collect();
        parsed_data = serde_json::json!(filtered);
    }

    Ok(ToolResult {
        success: true,
        output: parsed_data,
        error: None,
        duration_ms: 0,
    })
}

async fn tool_yaml_parse(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::DataManipulation)?;

    let content = input["content"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "yaml_parse".into(),
        message: "missing 'content' parameter".into(),
    })?;

    let parsed: serde_json::Value =
        serde_yaml::from_str(content).map_err(|e| PunchError::Tool {
            tool: "yaml_parse".into(),
            message: format!("failed to parse YAML: {}", e),
        })?;

    Ok(ToolResult {
        success: true,
        output: parsed,
        error: None,
        duration_ms: 0,
    })
}

async fn tool_regex_match(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::DataManipulation)?;

    let pattern_str = input["pattern"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "regex_match".into(),
        message: "missing 'pattern' parameter".into(),
    })?;
    let text = input["text"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "regex_match".into(),
        message: "missing 'text' parameter".into(),
    })?;
    let global = input["global"].as_bool().unwrap_or(false);

    let re = regex::Regex::new(pattern_str).map_err(|e| PunchError::Tool {
        tool: "regex_match".into(),
        message: format!("invalid regex: {}", e),
    })?;

    if global {
        let matches: Vec<serde_json::Value> = re
            .captures_iter(text)
            .map(|cap| {
                let groups: Vec<serde_json::Value> = cap
                    .iter()
                    .map(|m| m.map_or(serde_json::json!(null), |m| serde_json::json!(m.as_str())))
                    .collect();
                serde_json::json!(groups)
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: serde_json::json!({ "matches": matches }),
            error: None,
            duration_ms: 0,
        })
    } else if let Some(cap) = re.captures(text) {
        let groups: Vec<serde_json::Value> = cap
            .iter()
            .map(|m| m.map_or(serde_json::json!(null), |m| serde_json::json!(m.as_str())))
            .collect();

        Ok(ToolResult {
            success: true,
            output: serde_json::json!({ "matched": true, "groups": groups }),
            error: None,
            duration_ms: 0,
        })
    } else {
        Ok(ToolResult {
            success: true,
            output: serde_json::json!({ "matched": false, "groups": [] }),
            error: None,
            duration_ms: 0,
        })
    }
}

async fn tool_regex_replace(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::DataManipulation)?;

    let pattern_str = input["pattern"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "regex_replace".into(),
        message: "missing 'pattern' parameter".into(),
    })?;
    let replacement = input["replacement"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "regex_replace".into(),
            message: "missing 'replacement' parameter".into(),
        })?;
    let text = input["text"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "regex_replace".into(),
        message: "missing 'text' parameter".into(),
    })?;

    let re = regex::Regex::new(pattern_str).map_err(|e| PunchError::Tool {
        tool: "regex_replace".into(),
        message: format!("invalid regex: {}", e),
    })?;

    let result = re.replace_all(text, replacement).to_string();

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(result),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Process tool implementations
// ---------------------------------------------------------------------------

async fn tool_process_list(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::ShellExec("*".to_string()))?;

    let filter = input["filter"].as_str();

    // Use `ps` to get process list in a portable manner.
    let output = Command::new("ps")
        .args(["aux"])
        .current_dir(&context.working_dir)
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "process_list".into(),
            message: format!("failed to run ps: {}", e),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let lines: Vec<&str> = stdout.lines().collect();

    let header = lines.first().copied().unwrap_or("");
    let processes: Vec<serde_json::Value> = lines
        .iter()
        .skip(1)
        .filter(|line| {
            if let Some(f) = filter {
                line.contains(f)
            } else {
                true
            }
        })
        .take(100) // Limit output
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            serde_json::json!({
                "user": parts.first().unwrap_or(&""),
                "pid": parts.get(1).unwrap_or(&""),
                "cpu": parts.get(2).unwrap_or(&""),
                "mem": parts.get(3).unwrap_or(&""),
                "command": parts.get(10..).map(|s| s.join(" ")).unwrap_or_default(),
            })
        })
        .collect();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!({
            "header": header,
            "processes": processes,
            "count": processes.len(),
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_process_kill(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::ShellExec("*".to_string()))?;

    let pid = input["pid"].as_u64().ok_or_else(|| PunchError::Tool {
        tool: "process_kill".into(),
        message: "missing 'pid' parameter".into(),
    })?;

    let signal = input["signal"].as_str().unwrap_or("TERM");

    let output = Command::new("kill")
        .args([&format!("-{}", signal), &pid.to_string()])
        .output()
        .await
        .map_err(|e| PunchError::Tool {
            tool: "process_kill".into(),
            message: format!("failed to run kill: {}", e),
        })?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ToolResult {
        success: output.status.success(),
        output: serde_json::json!({
            "pid": pid,
            "signal": signal,
            "killed": output.status.success(),
        }),
        error: if output.status.success() {
            None
        } else {
            Some(format!("kill failed: {}", stderr))
        },
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Schedule tool implementations (in-memory DashMap scheduler)
// ---------------------------------------------------------------------------

/// Represents a scheduled task entry.
#[derive(Clone, Debug, serde::Serialize)]
struct ScheduledTask {
    id: String,
    name: String,
    command: String,
    delay_secs: u64,
    interval_secs: Option<u64>,
    status: String,
}

/// Global in-memory task registry.
static SCHEDULED_TASKS: LazyLock<DashMap<String, ScheduledTask>> = LazyLock::new(DashMap::new);

/// Global cancellation token registry.
static TASK_CANCELLERS: LazyLock<DashMap<String, tokio::sync::watch::Sender<bool>>> =
    LazyLock::new(DashMap::new);

async fn tool_schedule_task(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Schedule)?;

    let name = input["name"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "schedule_task".into(),
        message: "missing 'name' parameter".into(),
    })?;
    let command = input["command"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "schedule_task".into(),
        message: "missing 'command' parameter".into(),
    })?;
    let delay_secs = input["delay_secs"]
        .as_u64()
        .ok_or_else(|| PunchError::Tool {
            tool: "schedule_task".into(),
            message: "missing 'delay_secs' parameter".into(),
        })?;
    let interval_secs = input["interval_secs"].as_u64();

    let task_id = uuid::Uuid::new_v4().to_string();
    let task = ScheduledTask {
        id: task_id.clone(),
        name: name.to_string(),
        command: command.to_string(),
        delay_secs,
        interval_secs,
        status: "scheduled".to_string(),
    };

    SCHEDULED_TASKS.insert(task_id.clone(), task);

    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    TASK_CANCELLERS.insert(task_id.clone(), cancel_tx);

    // Spawn the delayed task.
    let task_id_clone = task_id.clone();
    let command_owned = command.to_string();
    let working_dir = context.working_dir.clone();

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;

        loop {
            if *cancel_rx.borrow() {
                break;
            }

            // Execute the command.
            let _output = Command::new("sh")
                .arg("-c")
                .arg(&command_owned)
                .current_dir(&working_dir)
                .output()
                .await;

            // Update task status.
            if let Some(mut entry) = SCHEDULED_TASKS.get_mut(&task_id_clone) {
                entry.status = "executed".to_string();
            }

            // If not recurring, exit.
            let Some(interval) = interval_secs else {
                break;
            };

            // Wait for the next interval or cancellation.
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(interval)) => {}
                _ = cancel_rx.changed() => {
                    break;
                }
            }
        }

        // Clean up on completion.
        if interval_secs.is_none() {
            SCHEDULED_TASKS.remove(&task_id_clone);
            TASK_CANCELLERS.remove(&task_id_clone);
        }
    });

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "task_id": task_id,
            "name": name,
            "delay_secs": delay_secs,
            "interval_secs": interval_secs,
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_schedule_list(capabilities: &[Capability]) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Schedule)?;

    let tasks: Vec<serde_json::Value> = SCHEDULED_TASKS
        .iter()
        .map(|entry| {
            let task = entry.value();
            serde_json::json!({
                "id": task.id,
                "name": task.name,
                "command": task.command,
                "delay_secs": task.delay_secs,
                "interval_secs": task.interval_secs,
                "status": task.status,
            })
        })
        .collect();

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(tasks),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_schedule_cancel(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Schedule)?;

    let task_id = input["task_id"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "schedule_cancel".into(),
        message: "missing 'task_id' parameter".into(),
    })?;

    // Send cancellation signal.
    if let Some(sender) = TASK_CANCELLERS.get(task_id) {
        let _ = sender.send(true);
    }

    // Remove from registries.
    let removed = SCHEDULED_TASKS.remove(task_id).is_some();
    TASK_CANCELLERS.remove(task_id);

    Ok(ToolResult {
        success: removed,
        output: serde_json::json!({
            "task_id": task_id,
            "cancelled": removed,
        }),
        error: if removed {
            None
        } else {
            Some(format!("task '{}' not found", task_id))
        },
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Code analysis tool implementations
// ---------------------------------------------------------------------------

async fn tool_code_search(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::CodeAnalysis)?;

    let pattern_str = input["pattern"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "code_search".into(),
        message: "missing 'pattern' parameter".into(),
    })?;
    let search_path = input["path"].as_str().unwrap_or(".");
    let file_pattern = input["file_pattern"].as_str();
    let max_results = input["max_results"].as_u64().unwrap_or(50) as usize;

    let resolved_path = resolve_path(&context.working_dir, search_path)?;

    let re = regex::Regex::new(pattern_str).map_err(|e| PunchError::Tool {
        tool: "code_search".into(),
        message: format!("invalid regex: {}", e),
    })?;

    let file_glob = file_pattern.and_then(|p| glob::Pattern::new(p).ok());

    let mut results = Vec::new();

    for entry in walkdir::WalkDir::new(&resolved_path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if results.len() >= max_results {
            break;
        }

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Apply file pattern filter.
        if let Some(ref glob_pat) = file_glob
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
            && !glob_pat.matches(name)
        {
            continue;
        }

        // Skip binary files (check first 512 bytes).
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        for (line_num, line) in content.lines().enumerate() {
            if results.len() >= max_results {
                break;
            }
            if re.is_match(line) {
                let rel_path = path
                    .strip_prefix(&resolved_path)
                    .unwrap_or(path)
                    .display()
                    .to_string();
                results.push(serde_json::json!({
                    "file": rel_path,
                    "line": line_num + 1,
                    "text": line.chars().take(200).collect::<String>(),
                }));
            }
        }
    }

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "matches": results,
            "count": results.len(),
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_code_symbols(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::CodeAnalysis)?;

    let path_str = input["path"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "code_symbols".into(),
        message: "missing 'path' parameter".into(),
    })?;

    let path = resolve_path(&context.working_dir, path_str)?;

    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| PunchError::Tool {
            tool: "code_symbols".into(),
            message: format!("failed to read '{}': {}", path.display(), e),
        })?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    // Regex patterns for common languages.
    let patterns: Vec<(&str, &str)> = match ext {
        "rs" => vec![
            ("function", r"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)"),
            ("struct", r"(?m)^\s*(?:pub\s+)?struct\s+(\w+)"),
            ("enum", r"(?m)^\s*(?:pub\s+)?enum\s+(\w+)"),
            ("trait", r"(?m)^\s*(?:pub\s+)?trait\s+(\w+)"),
            ("impl", r"(?m)^\s*impl(?:<[^>]*>)?\s+(\w+)"),
        ],
        "py" => vec![
            ("function", r"(?m)^\s*def\s+(\w+)"),
            ("class", r"(?m)^\s*class\s+(\w+)"),
        ],
        "js" | "ts" | "jsx" | "tsx" => vec![
            (
                "function",
                r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+(\w+)",
            ),
            ("class", r"(?m)^\s*(?:export\s+)?class\s+(\w+)"),
            (
                "const_fn",
                r"(?m)^\s*(?:export\s+)?const\s+(\w+)\s*=\s*(?:async\s+)?(?:\([^)]*\)|[^=])\s*=>",
            ),
        ],
        "go" => vec![
            ("function", r"(?m)^func\s+(?:\([^)]*\)\s+)?(\w+)"),
            ("type", r"(?m)^type\s+(\w+)\s+struct"),
            ("interface", r"(?m)^type\s+(\w+)\s+interface"),
        ],
        "java" | "kt" => vec![
            (
                "class",
                r"(?m)^\s*(?:public|private|protected)?\s*(?:static\s+)?class\s+(\w+)",
            ),
            (
                "method",
                r"(?m)^\s*(?:public|private|protected)?\s*(?:static\s+)?\w+\s+(\w+)\s*\(",
            ),
        ],
        _ => vec![
            (
                "function",
                r"(?m)^\s*(?:pub\s+)?(?:async\s+)?(?:fn|function|def)\s+(\w+)",
            ),
            ("class", r"(?m)^\s*(?:pub\s+)?(?:class|struct|enum)\s+(\w+)"),
        ],
    };

    let mut symbols = Vec::new();

    for (kind, pattern) in patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            for cap in re.captures_iter(&content) {
                if let Some(name_match) = cap.get(1) {
                    // Find line number.
                    let byte_offset = name_match.start();
                    let line_num = content[..byte_offset].matches('\n').count() + 1;
                    symbols.push(serde_json::json!({
                        "kind": kind,
                        "name": name_match.as_str(),
                        "line": line_num,
                    }));
                }
            }
        }
    }

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "file": path_str,
            "symbols": symbols,
            "count": symbols.len(),
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Archive tool implementations
// ---------------------------------------------------------------------------

async fn tool_archive_create(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Archive)?;

    let output_path_str = input["output_path"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "archive_create".into(),
            message: "missing 'output_path' parameter".into(),
        })?;
    let paths = input["paths"].as_array().ok_or_else(|| PunchError::Tool {
        tool: "archive_create".into(),
        message: "missing 'paths' parameter".into(),
    })?;

    let output_path = resolve_path(&context.working_dir, output_path_str)?;

    // Ensure parent directory exists.
    if let Some(parent) = output_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|e| PunchError::Tool {
            tool: "archive_create".into(),
            message: format!("failed to create directory: {}", e),
        })?;
    }

    let file = std::fs::File::create(&output_path).map_err(|e| PunchError::Tool {
        tool: "archive_create".into(),
        message: format!("failed to create archive file: {}", e),
    })?;

    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(enc);

    let mut file_count = 0u64;
    for path_val in paths {
        let Some(path_str) = path_val.as_str() else {
            continue;
        };
        let resolved = resolve_path(&context.working_dir, path_str)?;
        if resolved.is_dir() {
            builder
                .append_dir_all(path_str, &resolved)
                .map_err(|e| PunchError::Tool {
                    tool: "archive_create".into(),
                    message: format!("failed to add directory '{}': {}", path_str, e),
                })?;
            file_count += 1;
        } else if resolved.is_file() {
            builder
                .append_path_with_name(&resolved, path_str)
                .map_err(|e| PunchError::Tool {
                    tool: "archive_create".into(),
                    message: format!("failed to add file '{}': {}", path_str, e),
                })?;
            file_count += 1;
        }
    }

    builder.finish().map_err(|e| PunchError::Tool {
        tool: "archive_create".into(),
        message: format!("failed to finalize archive: {}", e),
    })?;

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "archive": output_path.display().to_string(),
            "entries": file_count,
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_archive_extract(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Archive)?;

    let archive_path_str = input["archive_path"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "archive_extract".into(),
            message: "missing 'archive_path' parameter".into(),
        })?;
    let destination_str = input["destination"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "archive_extract".into(),
            message: "missing 'destination' parameter".into(),
        })?;

    let archive_path = resolve_path(&context.working_dir, archive_path_str)?;
    let destination = resolve_path(&context.working_dir, destination_str)?;

    let file = std::fs::File::open(&archive_path).map_err(|e| PunchError::Tool {
        tool: "archive_extract".into(),
        message: format!("failed to open archive: {}", e),
    })?;

    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    std::fs::create_dir_all(&destination).map_err(|e| PunchError::Tool {
        tool: "archive_extract".into(),
        message: format!("failed to create destination directory: {}", e),
    })?;

    archive.unpack(&destination).map_err(|e| PunchError::Tool {
        tool: "archive_extract".into(),
        message: format!("failed to extract archive: {}", e),
    })?;

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "destination": destination.display().to_string(),
            "message": "archive extracted successfully",
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_archive_list(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Archive)?;

    let archive_path_str = input["archive_path"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "archive_list".into(),
            message: "missing 'archive_path' parameter".into(),
        })?;

    let archive_path = resolve_path(&context.working_dir, archive_path_str)?;

    let file = std::fs::File::open(&archive_path).map_err(|e| PunchError::Tool {
        tool: "archive_list".into(),
        message: format!("failed to open archive: {}", e),
    })?;

    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    let mut entries_list = Vec::new();
    for entry in archive.entries().map_err(|e| PunchError::Tool {
        tool: "archive_list".into(),
        message: format!("failed to read archive entries: {}", e),
    })? {
        let entry = entry.map_err(|e| PunchError::Tool {
            tool: "archive_list".into(),
            message: format!("failed to read entry: {}", e),
        })?;
        let path = entry
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<invalid path>".to_string());
        let size = entry.size();
        let is_dir = entry.header().entry_type().is_dir();
        entries_list.push(serde_json::json!({
            "path": path,
            "size": size,
            "is_directory": is_dir,
        }));
    }

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "entries": entries_list,
            "count": entries_list.len(),
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Template tool implementation
// ---------------------------------------------------------------------------

async fn tool_template_render(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Template)?;

    let template = input["template"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "template_render".into(),
        message: "missing 'template' parameter".into(),
    })?;
    let variables = input["variables"]
        .as_object()
        .ok_or_else(|| PunchError::Tool {
            tool: "template_render".into(),
            message: "missing 'variables' parameter (must be an object)".into(),
        })?;

    // Simple {{variable}} substitution using regex.
    let re = regex::Regex::new(r"\{\{(\w+)\}\}").map_err(|e| PunchError::Tool {
        tool: "template_render".into(),
        message: format!("internal regex error: {}", e),
    })?;

    let rendered = re.replace_all(template, |caps: &regex::Captures| {
        let var_name = &caps[1];
        variables
            .get(var_name)
            .map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    v.to_string()
                }
            })
            .unwrap_or_else(|| format!("{{{{{}}}}}", var_name))
    });

    Ok(ToolResult {
        success: true,
        output: serde_json::json!(rendered.to_string()),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Crypto / Hash tool implementations
// ---------------------------------------------------------------------------

/// Compute the hex-encoded hash of bytes using the specified algorithm.
fn compute_hash(algorithm: &str, data: &[u8]) -> PunchResult<String> {
    use sha2::Digest;
    match algorithm {
        "sha256" => {
            let mut hasher = sha2::Sha256::new();
            hasher.update(data);
            Ok(format!("{:x}", hasher.finalize()))
        }
        "sha512" => {
            let mut hasher = sha2::Sha512::new();
            hasher.update(data);
            Ok(format!("{:x}", hasher.finalize()))
        }
        "md5" => {
            // Simple MD5 implementation note: MD5 is cryptographically insecure.
            // We compute it via shell `md5sum` or `md5` for portability.
            // For in-process computation, we use a basic implementation.
            // Since we don't have an md5 crate, compute via sha256 as fallback
            // with a clear message. For now, shell out to md5sum.
            Err(PunchError::Tool {
                tool: "hash_compute".into(),
                message: "MD5 is not supported in-process (insecure and deprecated). Use sha256 or sha512 instead.".into(),
            })
        }
        other => Err(PunchError::Tool {
            tool: "hash_compute".into(),
            message: format!("unsupported algorithm '{}', use sha256 or sha512", other),
        }),
    }
}

async fn tool_hash_compute(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Crypto)?;

    let algorithm = input["algorithm"].as_str().unwrap_or("sha256");

    let data = if let Some(input_str) = input["input"].as_str() {
        input_str.as_bytes().to_vec()
    } else if let Some(file_path) = input["file"].as_str() {
        let path = resolve_path(&context.working_dir, file_path)?;
        std::fs::read(&path).map_err(|e| PunchError::Tool {
            tool: "hash_compute".into(),
            message: format!("failed to read file '{}': {}", path.display(), e),
        })?
    } else {
        return Ok(ToolResult {
            success: false,
            output: serde_json::json!(null),
            error: Some("must provide either 'input' (string) or 'file' (path) parameter".into()),
            duration_ms: 0,
        });
    };

    let hash = compute_hash(algorithm, &data)?;

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "algorithm": algorithm,
            "hash": hash,
            "bytes_hashed": data.len(),
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_hash_verify(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::Crypto)?;

    let algorithm = input["algorithm"].as_str().unwrap_or("sha256");
    let expected = input["expected"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "hash_verify".into(),
        message: "missing 'expected' parameter".into(),
    })?;

    let data = if let Some(input_str) = input["input"].as_str() {
        input_str.as_bytes().to_vec()
    } else if let Some(file_path) = input["file"].as_str() {
        let path = resolve_path(&context.working_dir, file_path)?;
        std::fs::read(&path).map_err(|e| PunchError::Tool {
            tool: "hash_verify".into(),
            message: format!("failed to read file '{}': {}", path.display(), e),
        })?
    } else {
        return Ok(ToolResult {
            success: false,
            output: serde_json::json!(null),
            error: Some("must provide either 'input' (string) or 'file' (path) parameter".into()),
            duration_ms: 0,
        });
    };

    let actual = compute_hash(algorithm, &data)?;
    let matches = actual.eq_ignore_ascii_case(expected);

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "algorithm": algorithm,
            "expected": expected,
            "actual": actual,
            "matches": matches,
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Environment tool implementations
// ---------------------------------------------------------------------------

async fn tool_env_get(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::ShellExec("*".to_string()))?;

    let name = input["name"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "env_get".into(),
        message: "missing 'name' parameter".into(),
    })?;

    match std::env::var(name) {
        Ok(value) => Ok(ToolResult {
            success: true,
            output: serde_json::json!({
                "name": name,
                "value": value,
            }),
            error: None,
            duration_ms: 0,
        }),
        Err(_) => Ok(ToolResult {
            success: true,
            output: serde_json::json!({
                "name": name,
                "value": null,
                "message": format!("environment variable '{}' is not set", name),
            }),
            error: None,
            duration_ms: 0,
        }),
    }
}

async fn tool_env_list(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::ShellExec("*".to_string()))?;

    let prefix = input["prefix"].as_str();

    let vars: Vec<serde_json::Value> = std::env::vars()
        .filter(|(key, _)| {
            if let Some(p) = prefix {
                key.starts_with(p)
            } else {
                true
            }
        })
        .map(|(key, value)| {
            serde_json::json!({
                "name": key,
                "value": value,
            })
        })
        .collect();

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "variables": vars,
            "count": vars.len(),
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// Text tool implementations
// ---------------------------------------------------------------------------

async fn tool_text_diff(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::DataManipulation)?;

    let old_text = input["old_text"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "text_diff".into(),
        message: "missing 'old_text' parameter".into(),
    })?;
    let new_text = input["new_text"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "text_diff".into(),
        message: "missing 'new_text' parameter".into(),
    })?;
    let label = input["label"].as_str().unwrap_or("file");

    let diff = punch_types::generate_unified_diff(old_text, new_text, label, label);

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "diff": diff,
            "has_changes": !diff.is_empty() && diff.contains("@@"),
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_text_count(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::DataManipulation)?;

    let text = input["text"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "text_count".into(),
        message: "missing 'text' parameter".into(),
    })?;

    let lines = text.lines().count();
    let words = text.split_whitespace().count();
    let characters = text.chars().count();
    let bytes = text.len();

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "lines": lines,
            "words": words,
            "characters": characters,
            "bytes": bytes,
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// File tool implementations (extended)
// ---------------------------------------------------------------------------

async fn tool_file_search(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    // File search requires file read capability.
    require_capability(capabilities, &Capability::FileRead("**".to_string()))?;

    let pattern_str = input["pattern"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "file_search".into(),
        message: "missing 'pattern' parameter".into(),
    })?;
    let search_path = input["path"].as_str().unwrap_or(".");
    let max_results = input["max_results"].as_u64().unwrap_or(100) as usize;

    let resolved_path = resolve_path(&context.working_dir, search_path)?;

    let glob_pat = glob::Pattern::new(pattern_str).map_err(|e| PunchError::Tool {
        tool: "file_search".into(),
        message: format!("invalid glob pattern: {}", e),
    })?;

    let mut results = Vec::new();

    for entry in walkdir::WalkDir::new(&resolved_path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if results.len() >= max_results {
            break;
        }

        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && glob_pat.matches(name)
        {
            let rel_path = path
                .strip_prefix(&resolved_path)
                .unwrap_or(path)
                .display()
                .to_string();
            let is_dir = path.is_dir();
            results.push(serde_json::json!({
                "path": rel_path,
                "is_directory": is_dir,
            }));
        }
    }

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "matches": results,
            "count": results.len(),
        }),
        error: None,
        duration_ms: 0,
    })
}

async fn tool_file_info(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    let path_str = input["path"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "file_info".into(),
        message: "missing 'path' parameter".into(),
    })?;

    let path = resolve_path(&context.working_dir, path_str)?;
    let path_display = path.display().to_string();

    require_capability(capabilities, &Capability::FileRead(path_display.clone()))?;

    let metadata = std::fs::metadata(&path).map_err(|e| PunchError::Tool {
        tool: "file_info".into(),
        message: format!("failed to get metadata for '{}': {}", path_display, e),
    })?;

    let file_type = if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_symlink() {
        "symlink"
    } else {
        "other"
    };

    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        })
        .unwrap_or(0);

    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        format!("{:o}", metadata.permissions().mode())
    };
    #[cfg(not(unix))]
    let permissions = if metadata.permissions().readonly() {
        "readonly".to_string()
    } else {
        "read-write".to_string()
    };

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "path": path_display,
            "type": file_type,
            "size_bytes": metadata.len(),
            "modified_unix": modified,
            "permissions": permissions,
            "readonly": metadata.permissions().readonly(),
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// WASM Plugin Invocation
// ---------------------------------------------------------------------------

/// Invoke a function on a loaded WASM plugin (imported technique).
///
/// Requires the `PluginInvoke` capability and a configured plugin registry.
/// Looks up the plugin by name, then delegates to `PluginRegistry::invoke`.
async fn tool_wasm_invoke(
    input: &serde_json::Value,
    capabilities: &[Capability],
    context: &ToolExecutionContext,
) -> PunchResult<ToolResult> {
    require_capability(capabilities, &Capability::PluginInvoke)?;

    let registry = context
        .plugin_registry
        .as_ref()
        .ok_or_else(|| PunchError::Tool {
            tool: "wasm_invoke".into(),
            message: "plugin runtime not configured — no imported techniques available".into(),
        })?;

    let plugin_name = input["plugin"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "wasm_invoke".into(),
        message: "missing 'plugin' parameter".into(),
    })?;

    let function = input["function"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "wasm_invoke".into(),
        message: "missing 'function' parameter".into(),
    })?;

    let args = input.get("input").cloned().unwrap_or(serde_json::json!({}));

    // Look up the plugin by name.
    let plugin_instance = registry
        .get_by_name(plugin_name)
        .ok_or_else(|| PunchError::Tool {
            tool: "wasm_invoke".into(),
            message: format!("plugin '{}' not found in registry", plugin_name),
        })?;

    let plugin_input = punch_extensions::plugin::PluginInput {
        function: function.to_string(),
        args,
        context: serde_json::json!({
            "fighter_id": context.fighter_id.to_string(),
        }),
    };

    let output = registry.invoke(&plugin_instance.id, plugin_input).await?;

    debug!(
        plugin = %plugin_name,
        function = %function,
        execution_ms = output.execution_ms,
        "wasm_invoke: technique executed"
    );

    Ok(ToolResult {
        success: true,
        output: serde_json::json!({
            "result": output.result,
            "logs": output.logs,
            "execution_ms": output.execution_ms,
            "memory_used_bytes": output.memory_used_bytes,
        }),
        error: None,
        duration_ms: 0,
    })
}

// ---------------------------------------------------------------------------
// A2A Delegation
// ---------------------------------------------------------------------------

/// Default timeout for A2A delegation polling (60 seconds).
const A2A_DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Polling interval when waiting for a delegated A2A task to complete.
const A2A_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Delegate a task to a remote A2A agent.
///
/// Discovers the remote agent via its well-known card URL, sends a task using
/// the A2A protocol, polls for completion (with timeout), and returns the
/// result. Works standalone — only needs HTTP, no Ring required.
async fn tool_a2a_delegate(
    input: &serde_json::Value,
    capabilities: &[Capability],
) -> PunchResult<ToolResult> {
    use punch_types::a2a::{A2AClient, A2ATask, A2ATaskInput, A2ATaskStatus, HttpA2AClient};

    require_capability(capabilities, &Capability::A2ADelegate)?;

    // --- Parse input arguments ---
    let agent_url = input["agent_url"]
        .as_str()
        .ok_or_else(|| PunchError::Tool {
            tool: "a2a_delegate".into(),
            message: "missing 'agent_url' parameter".into(),
        })?;

    let prompt = input["prompt"].as_str().ok_or_else(|| PunchError::Tool {
        tool: "a2a_delegate".into(),
        message: "missing 'prompt' parameter".into(),
    })?;

    let timeout_secs = input["timeout_secs"]
        .as_u64()
        .unwrap_or(A2A_DEFAULT_TIMEOUT_SECS);

    let context = input["context"].as_object().cloned().unwrap_or_default();

    // --- Build the client with the configured timeout ---
    let client = HttpA2AClient::with_timeout(std::time::Duration::from_secs(timeout_secs))
        .map_err(|e| PunchError::Tool {
            tool: "a2a_delegate".into(),
            message: format!("failed to create A2A client: {e}"),
        })?;

    // --- Discover the remote agent ---
    let agent_card = client
        .discover(agent_url)
        .await
        .map_err(|e| PunchError::Tool {
            tool: "a2a_delegate".into(),
            message: format!("agent discovery failed for {agent_url}: {e}"),
        })?;

    debug!(
        agent = %agent_card.name,
        url = %agent_url,
        "a2a_delegate: discovered remote agent"
    );

    // --- Build and send the task ---
    let task_input = A2ATaskInput {
        prompt: prompt.to_string(),
        context,
        mode: "text".to_string(),
    };

    let now = chrono::Utc::now();
    let task = A2ATask {
        id: uuid::Uuid::new_v4().to_string(),
        status: A2ATaskStatus::Pending,
        input: serde_json::to_value(&task_input).unwrap_or(serde_json::json!({})),
        output: None,
        created_at: now,
        updated_at: now,
    };

    let sent_task = client
        .send_task(&agent_card, task)
        .await
        .map_err(|e| PunchError::Tool {
            tool: "a2a_delegate".into(),
            message: format!("failed to send task to '{}': {e}", agent_card.name),
        })?;

    let task_id = sent_task.id.clone();

    debug!(
        task_id = %task_id,
        agent = %agent_card.name,
        "a2a_delegate: task sent"
    );

    // --- Poll for completion ---
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    // If the task came back already in a terminal state, skip polling.
    let final_status = match &sent_task.status {
        A2ATaskStatus::Completed | A2ATaskStatus::Failed(_) | A2ATaskStatus::Cancelled => {
            sent_task.status.clone()
        }
        _ => loop {
            if tokio::time::Instant::now() >= deadline {
                // Best-effort cancellation of the timed-out task.
                let _ = client.cancel_task(&agent_card, &task_id).await;
                return Ok(ToolResult {
                    success: false,
                    output: serde_json::json!({
                        "agent": agent_card.name,
                        "task_id": task_id,
                        "error": format!("task timed out after {timeout_secs}s"),
                    }),
                    error: Some(format!(
                        "A2A delegation to '{}' timed out after {}s",
                        agent_card.name, timeout_secs
                    )),
                    duration_ms: 0,
                });
            }

            tokio::time::sleep(A2A_POLL_INTERVAL).await;

            match client.get_task_status(&agent_card, &task_id).await {
                Ok(
                    status @ (A2ATaskStatus::Completed
                    | A2ATaskStatus::Failed(_)
                    | A2ATaskStatus::Cancelled),
                ) => break status,
                Ok(_) => continue,
                Err(e) => {
                    warn!(
                        task_id = %task_id,
                        agent = %agent_card.name,
                        error = %e,
                        "a2a_delegate: status poll failed, retrying"
                    );
                    continue;
                }
            }
        },
    };

    // --- Build the result ---
    match final_status {
        A2ATaskStatus::Completed => {
            let output = sent_task.output.unwrap_or(serde_json::json!(null));
            Ok(ToolResult {
                success: true,
                output: serde_json::json!({
                    "agent": agent_card.name,
                    "task_id": task_id,
                    "status": "completed",
                    "output": output,
                }),
                error: None,
                duration_ms: 0,
            })
        }
        A2ATaskStatus::Failed(ref msg) => Ok(ToolResult {
            success: false,
            output: serde_json::json!({
                "agent": agent_card.name,
                "task_id": task_id,
                "status": "failed",
                "error": msg,
            }),
            error: Some(format!("A2A task on '{}' failed: {}", agent_card.name, msg)),
            duration_ms: 0,
        }),
        A2ATaskStatus::Cancelled => Ok(ToolResult {
            success: false,
            output: serde_json::json!({
                "agent": agent_card.name,
                "task_id": task_id,
                "status": "cancelled",
            }),
            error: Some(format!("A2A task on '{}' was cancelled", agent_card.name)),
            duration_ms: 0,
        }),
        _ => Ok(ToolResult {
            success: false,
            output: serde_json::json!({
                "agent": agent_card.name,
                "task_id": task_id,
                "status": "unknown",
            }),
            error: Some(format!(
                "A2A task on '{}' ended in unexpected state",
                agent_card.name
            )),
            duration_ms: 0,
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use punch_types::{
        AgentCoordinator, AgentInfo, AgentMessageResult, Capability, FighterId, FighterManifest,
        FighterStatus,
    };

    /// A mock coordinator for testing agent tools.
    struct MockCoordinator {
        fighters: Vec<AgentInfo>,
    }

    impl MockCoordinator {
        fn new() -> Self {
            Self {
                fighters: vec![AgentInfo {
                    id: FighterId(uuid::Uuid::nil()),
                    name: "test-fighter".to_string(),
                    status: FighterStatus::Idle,
                }],
            }
        }
    }

    #[async_trait]
    impl AgentCoordinator for MockCoordinator {
        async fn spawn_fighter(&self, _manifest: FighterManifest) -> PunchResult<FighterId> {
            Ok(FighterId(uuid::Uuid::new_v4()))
        }

        async fn send_message_to_agent(
            &self,
            _target: &FighterId,
            message: String,
        ) -> PunchResult<AgentMessageResult> {
            Ok(AgentMessageResult {
                response: format!("echo: {}", message),
                tokens_used: 42,
            })
        }

        async fn find_fighter_by_name(&self, name: &str) -> PunchResult<Option<FighterId>> {
            let found = self.fighters.iter().find(|f| f.name == name).map(|f| f.id);
            Ok(found)
        }

        async fn list_fighters(&self) -> PunchResult<Vec<AgentInfo>> {
            Ok(self.fighters.clone())
        }
    }

    fn make_test_context(coordinator: Option<Arc<dyn AgentCoordinator>>) -> ToolExecutionContext {
        ToolExecutionContext {
            working_dir: std::env::temp_dir(),
            fighter_id: FighterId(uuid::Uuid::new_v4()),
            memory: Arc::new(MemorySubstrate::in_memory().unwrap()),
            coordinator,
            approval_engine: None,
            sandbox: None,
            bleed_detector: None,
            browser_pool: None,
            plugin_registry: None,
            mcp_clients: None,
            channel_notifier: None,
        }
    }

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

    // -- Agent tool tests ---------------------------------------------------

    #[tokio::test]
    async fn test_agent_message_with_mock_coordinator() {
        let coordinator: Arc<dyn AgentCoordinator> = Arc::new(MockCoordinator::new());
        let context = make_test_context(Some(coordinator));
        let caps = vec![Capability::AgentMessage];
        let target_id = uuid::Uuid::nil().to_string();

        let input = serde_json::json!({
            "fighter_id": target_id,
            "message": "hello from fighter A"
        });

        let result = execute_tool("agent_message", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        let response = result.output["response"].as_str().unwrap();
        assert_eq!(response, "echo: hello from fighter A");
        assert_eq!(result.output["tokens_used"].as_u64().unwrap(), 42);
    }

    #[tokio::test]
    async fn test_agent_message_by_name() {
        let coordinator: Arc<dyn AgentCoordinator> = Arc::new(MockCoordinator::new());
        let context = make_test_context(Some(coordinator));
        let caps = vec![Capability::AgentMessage];

        let input = serde_json::json!({
            "name": "test-fighter",
            "message": "hello by name"
        });

        let result = execute_tool("agent_message", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(
            result.output["response"].as_str().unwrap(),
            "echo: hello by name"
        );
    }

    #[tokio::test]
    async fn test_agent_message_name_not_found() {
        let coordinator: Arc<dyn AgentCoordinator> = Arc::new(MockCoordinator::new());
        let context = make_test_context(Some(coordinator));
        let caps = vec![Capability::AgentMessage];

        let input = serde_json::json!({
            "name": "nonexistent-fighter",
            "message": "hello"
        });

        let result = execute_tool("agent_message", &input, &caps, &context)
            .await
            .unwrap();

        // Should fail gracefully (not panic).
        assert!(!result.success);
        assert!(result.error.unwrap().contains("nonexistent-fighter"));
    }

    #[tokio::test]
    async fn test_agent_list_with_mock_coordinator() {
        let coordinator: Arc<dyn AgentCoordinator> = Arc::new(MockCoordinator::new());
        let context = make_test_context(Some(coordinator));
        let caps = vec![Capability::AgentMessage];

        let input = serde_json::json!({});

        let result = execute_tool("agent_list", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        let agents = result.output.as_array().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["name"].as_str().unwrap(), "test-fighter");
    }

    #[tokio::test]
    async fn test_agent_spawn_with_mock_coordinator() {
        let coordinator: Arc<dyn AgentCoordinator> = Arc::new(MockCoordinator::new());
        let context = make_test_context(Some(coordinator));
        let caps = vec![Capability::AgentSpawn];

        let input = serde_json::json!({
            "name": "worker-1",
            "system_prompt": "You are a worker agent."
        });

        let result = execute_tool("agent_spawn", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["name"].as_str().unwrap(), "worker-1");
        assert!(result.output["fighter_id"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_agent_message_denied_without_capability() {
        let coordinator: Arc<dyn AgentCoordinator> = Arc::new(MockCoordinator::new());
        let context = make_test_context(Some(coordinator));
        // No AgentMessage capability.
        let caps = vec![Capability::Memory];

        let input = serde_json::json!({
            "fighter_id": uuid::Uuid::nil().to_string(),
            "message": "hello"
        });

        let result = execute_tool("agent_message", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("capability"));
    }

    #[tokio::test]
    async fn test_agent_spawn_denied_without_capability() {
        let coordinator: Arc<dyn AgentCoordinator> = Arc::new(MockCoordinator::new());
        let context = make_test_context(Some(coordinator));
        // No AgentSpawn capability.
        let caps = vec![Capability::Memory];

        let input = serde_json::json!({
            "name": "worker-1",
            "system_prompt": "test"
        });

        let result = execute_tool("agent_spawn", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("capability"));
    }

    #[test]
    fn test_parse_duckduckgo_results_mock_html() {
        let mock_html = r#"
        <div class="result">
            <a rel="nofollow" class="result__a" href="/l/?uddg=https%3A%2F%2Fexample.com%2Fpage1&rut=abc">
                <b>Example</b> Page One
            </a>
        </div>
        <div class="result">
            <a rel="nofollow" class="result__a" href="/l/?uddg=https%3A%2F%2Fexample.org%2Fpage2&rut=def">
                Example Page Two
            </a>
        </div>
        "#;

        let results = parse_duckduckgo_results(mock_html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["title"].as_str().unwrap(), "Example Page One");
        assert_eq!(
            results[0]["url"].as_str().unwrap(),
            "https://example.com/page1"
        );
        assert_eq!(results[1]["title"].as_str().unwrap(), "Example Page Two");
        assert_eq!(
            results[1]["url"].as_str().unwrap(),
            "https://example.org/page2"
        );
    }

    #[test]
    fn test_parse_duckduckgo_results_empty_html() {
        let results = parse_duckduckgo_results("<html><body>No results</body></html>");
        assert!(results.is_empty());
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<b>bold</b> text"), "bold text");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(strip_html_tags("<a href=\"x\">link</a>"), "link");
    }

    #[tokio::test]
    async fn test_agent_tools_without_coordinator() {
        let context = make_test_context(None);
        let caps = vec![Capability::AgentMessage];

        let input = serde_json::json!({
            "fighter_id": uuid::Uuid::nil().to_string(),
            "message": "hello"
        });

        let result = execute_tool("agent_message", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("coordinator not available"));
    }

    // -- Approval engine integration tests --

    #[tokio::test]
    async fn test_tool_call_blocked_by_approval_policy() {
        use punch_types::{ApprovalPolicy, DenyAllHandler, PolicyEngine, RiskLevel};

        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "block-file-reads".into(),
                tool_patterns: vec!["file_read".into()],
                risk_level: RiskLevel::High,
                auto_approve: false,
                max_auto_approvals: None,
            }],
            Arc::new(DenyAllHandler),
        );

        let mut context = make_test_context(None);
        context.approval_engine = Some(Arc::new(engine));

        let caps = vec![Capability::FileRead("**".into())];
        let input = serde_json::json!({"path": "/etc/passwd"});

        let result = execute_tool("file_read", &input, &caps, &context)
            .await
            .expect("execute_tool should not error");

        assert!(!result.success);
        let error = result.error.expect("should have error message");
        assert!(
            error.contains("denied by policy"),
            "expected 'denied by policy' in error, got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_tool_call_allowed_by_approval_policy() {
        use punch_types::{ApprovalPolicy, AutoApproveHandler, PolicyEngine, RiskLevel};

        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "allow-file-reads".into(),
                tool_patterns: vec!["file_read".into()],
                risk_level: RiskLevel::Low,
                auto_approve: true,
                max_auto_approvals: None,
            }],
            Arc::new(AutoApproveHandler),
        );

        let mut context = make_test_context(None);
        context.approval_engine = Some(Arc::new(engine));

        // Write a temp file to read.
        let temp_file = context.working_dir.join("punch_approval_test.txt");
        tokio::fs::write(&temp_file, "approval test content")
            .await
            .expect("write temp file");

        let caps = vec![Capability::FileRead("**".into())];
        let input = serde_json::json!({"path": temp_file.to_string_lossy()});

        let result = execute_tool("file_read", &input, &caps, &context)
            .await
            .expect("execute_tool should not error");

        assert!(
            result.success,
            "tool call should succeed: {:?}",
            result.error
        );

        // Clean up.
        let _ = tokio::fs::remove_file(&temp_file).await;
    }

    // -----------------------------------------------------------------------
    // Browser tool tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_browser_navigate_requires_capability() {
        let context = make_test_context(None);
        let caps = vec![Capability::Memory]; // no BrowserControl

        let input = serde_json::json!({"url": "https://example.com"});
        let result = execute_tool("browser_navigate", &input, &caps, &context)
            .await
            .expect("should not hard-error");

        assert!(!result.success);
        let error = result.error.expect("should have error");
        assert!(
            error.contains("capability denied") || error.contains("missing capability"),
            "expected capability denied, got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_browser_navigate_no_pool() {
        let context = make_test_context(None); // browser_pool is None
        let caps = vec![Capability::BrowserControl];

        let input = serde_json::json!({"url": "https://example.com"});
        let result = execute_tool("browser_navigate", &input, &caps, &context)
            .await
            .expect("should not hard-error");

        assert!(!result.success);
        let error = result.error.expect("should have error");
        assert!(
            error.contains("browser not available"),
            "expected 'browser not available', got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_browser_navigate_with_pool_no_driver() {
        use punch_types::{BrowserConfig, BrowserPool};

        let pool = Arc::new(BrowserPool::new(BrowserConfig::default(), 5));
        let mut context = make_test_context(None);
        context.browser_pool = Some(pool);

        let caps = vec![Capability::BrowserControl];
        let input = serde_json::json!({"url": "https://example.com"});

        let result = execute_tool("browser_navigate", &input, &caps, &context)
            .await
            .expect("should not hard-error");

        // Pool is available but no CDP driver, so the tool reports failure gracefully.
        assert!(!result.success);
        let error = result.error.expect("should have error");
        assert!(
            error.contains("no CDP driver"),
            "expected 'no CDP driver', got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_browser_screenshot_with_pool() {
        use punch_types::{BrowserConfig, BrowserPool};

        let pool = Arc::new(BrowserPool::new(BrowserConfig::default(), 5));
        let mut context = make_test_context(None);
        context.browser_pool = Some(pool);

        let caps = vec![Capability::BrowserControl];
        let input = serde_json::json!({"full_page": true});

        let result = execute_tool("browser_screenshot", &input, &caps, &context)
            .await
            .expect("should not hard-error");

        assert!(!result.success);
        assert_eq!(result.output["full_page"], true);
    }

    #[tokio::test]
    async fn test_browser_click_missing_selector() {
        use punch_types::{BrowserConfig, BrowserPool};

        let pool = Arc::new(BrowserPool::new(BrowserConfig::default(), 5));
        let mut context = make_test_context(None);
        context.browser_pool = Some(pool);

        let caps = vec![Capability::BrowserControl];
        let input = serde_json::json!({});

        let result = execute_tool("browser_click", &input, &caps, &context)
            .await
            .expect("should not hard-error");

        assert!(!result.success);
        let error = result.error.expect("should have error");
        assert!(
            error.contains("missing 'selector'"),
            "expected missing selector error, got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_browser_type_missing_params() {
        use punch_types::{BrowserConfig, BrowserPool};

        let pool = Arc::new(BrowserPool::new(BrowserConfig::default(), 5));
        let mut context = make_test_context(None);
        context.browser_pool = Some(pool);

        let caps = vec![Capability::BrowserControl];

        // Missing 'text' param
        let input = serde_json::json!({"selector": "#input"});
        let result = execute_tool("browser_type", &input, &caps, &context)
            .await
            .expect("should not hard-error");

        assert!(!result.success);
        let error = result.error.expect("should have error");
        assert!(error.contains("missing 'text'"), "got: {}", error);
    }

    #[tokio::test]
    async fn test_browser_content_with_pool() {
        use punch_types::{BrowserConfig, BrowserPool};

        let pool = Arc::new(BrowserPool::new(BrowserConfig::default(), 5));
        let mut context = make_test_context(None);
        context.browser_pool = Some(pool);

        let caps = vec![Capability::BrowserControl];
        let input = serde_json::json!({"selector": "h1"});

        let result = execute_tool("browser_content", &input, &caps, &context)
            .await
            .expect("should not hard-error");

        assert!(!result.success);
        assert_eq!(result.output["selector"], "h1");
    }

    // -----------------------------------------------------------------------
    // New tool tests — data manipulation, regex, code analysis
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_json_query_basic_path() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "data": {"users": [{"name": "Alice"}, {"name": "Bob"}]},
            "path": "users.1.name"
        });

        let result = execute_tool("json_query", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output, serde_json::json!("Bob"));
    }

    #[tokio::test]
    async fn test_regex_match_with_captures() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "pattern": r"(\d+)-(\d+)",
            "text": "order 123-456 confirmed",
            "global": false
        });

        let result = execute_tool("regex_match", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["matched"], true);
        let groups = result.output["groups"].as_array().unwrap();
        assert_eq!(groups[0], "123-456");
        assert_eq!(groups[1], "123");
        assert_eq!(groups[2], "456");
    }

    #[tokio::test]
    async fn test_regex_replace_basic() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "pattern": r"(\w+)@(\w+)",
            "replacement": "$1 AT $2",
            "text": "email user@example domain"
        });

        let result = execute_tool("regex_replace", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(
            result.output,
            serde_json::json!("email user AT example domain")
        );
    }

    #[tokio::test]
    async fn test_yaml_parse_basic() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "content": "name: Alice\nage: 30\ntags:\n  - rust\n  - python"
        });

        let result = execute_tool("yaml_parse", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["name"], "Alice");
        assert_eq!(result.output["age"], 30);
        let tags = result.output["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
    }

    #[tokio::test]
    async fn test_json_transform_extract_and_rename() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "data": [
                {"name": "Alice", "age": 30, "city": "NYC"},
                {"name": "Bob", "age": 25, "city": "LA"}
            ],
            "extract": ["name", "city"],
            "rename": {"name": "full_name"}
        });

        let result = execute_tool("json_transform", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        let arr = result.output.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["full_name"], "Alice");
        assert!(arr[0].get("age").is_none());
    }

    #[tokio::test]
    async fn test_code_symbols_rust_file() {
        let context = make_test_context(None);
        let caps = vec![Capability::CodeAnalysis];

        // Write a temp Rust file.
        let temp_file = context.working_dir.join("punch_test_symbols.rs");
        tokio::fs::write(
            &temp_file,
            "pub fn hello() {}\nstruct Foo {}\nasync fn bar() {}\nenum Color {}",
        )
        .await
        .unwrap();

        let input = serde_json::json!({
            "path": temp_file.to_string_lossy()
        });

        let result = execute_tool("code_symbols", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        let symbols = result.output["symbols"].as_array().unwrap();
        let names: Vec<&str> = symbols.iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(names.contains(&"hello"), "missing hello: {:?}", names);
        assert!(names.contains(&"Foo"), "missing Foo: {:?}", names);
        assert!(names.contains(&"bar"), "missing bar: {:?}", names);
        assert!(names.contains(&"Color"), "missing Color: {:?}", names);

        // Clean up.
        let _ = tokio::fs::remove_file(&temp_file).await;
    }

    // -----------------------------------------------------------------------
    // New tool tests — archive, template, hash, env, text, file
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_template_render_basic() {
        let context = make_test_context(None);
        let caps = vec![Capability::Template];

        let input = serde_json::json!({
            "template": "Hello, {{name}}! You are {{age}} years old.",
            "variables": {"name": "Alice", "age": 30}
        });

        let result = execute_tool("template_render", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output, "Hello, Alice! You are 30 years old.");
    }

    #[tokio::test]
    async fn test_hash_compute_sha256() {
        let context = make_test_context(None);
        let caps = vec![Capability::Crypto];

        let input = serde_json::json!({
            "algorithm": "sha256",
            "input": "hello world"
        });

        let result = execute_tool("hash_compute", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        let hash = result.output["hash"].as_str().unwrap();
        // Known SHA-256 of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[tokio::test]
    async fn test_hash_verify_match() {
        let context = make_test_context(None);
        let caps = vec![Capability::Crypto];

        let input = serde_json::json!({
            "algorithm": "sha256",
            "input": "hello world",
            "expected": "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        });

        let result = execute_tool("hash_verify", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["matches"], true);
    }

    #[tokio::test]
    async fn test_text_count_basic() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "text": "hello world\nfoo bar baz\n"
        });

        let result = execute_tool("text_count", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["lines"], 2);
        assert_eq!(result.output["words"], 5);
    }

    #[tokio::test]
    async fn test_text_diff_basic() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "old_text": "line1\nline2\nline3",
            "new_text": "line1\nchanged\nline3"
        });

        let result = execute_tool("text_diff", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["has_changes"], true);
        let diff = result.output["diff"].as_str().unwrap();
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+changed"));
    }

    #[tokio::test]
    async fn test_env_get_existing_var() {
        let context = make_test_context(None);
        let caps = vec![Capability::ShellExec("*".to_string())];

        // PATH should exist on all systems.
        let input = serde_json::json!({"name": "PATH"});

        let result = execute_tool("env_get", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output["value"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_file_info_basic() {
        let context = make_test_context(None);
        let caps = vec![Capability::FileRead("**".to_string())];

        // Write a temp file.
        let temp_file = context.working_dir.join("punch_file_info_test.txt");
        tokio::fs::write(&temp_file, "test content").await.unwrap();

        let input = serde_json::json!({
            "path": temp_file.to_string_lossy()
        });

        let result = execute_tool("file_info", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["type"], "file");
        assert_eq!(result.output["size_bytes"], 12); // "test content" = 12 bytes

        let _ = tokio::fs::remove_file(&temp_file).await;
    }

    #[tokio::test]
    async fn test_all_tools_count_at_least_55() {
        let tools = crate::tools::all_tools();
        assert!(
            tools.len() >= 55,
            "expected at least 55 tools, got {}",
            tools.len()
        );
    }

    // -----------------------------------------------------------------------
    // Tool dispatch coverage — verify every tool name routes correctly
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dispatch_unknown_tool() {
        let context = make_test_context(None);
        let caps = vec![Capability::Memory];
        let input = serde_json::json!({});

        let result = execute_tool("nonexistent_tool", &input, &caps, &context)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("nonexistent_tool"));
    }

    #[tokio::test]
    async fn test_dispatch_file_read_missing_path() {
        let context = make_test_context(None);
        let caps = vec![Capability::FileRead("**".into())];
        let input = serde_json::json!({});

        let result = execute_tool("file_read", &input, &caps, &context)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("missing 'path'"));
    }

    #[tokio::test]
    async fn test_dispatch_file_write_missing_params() {
        let context = make_test_context(None);
        let caps = vec![Capability::FileWrite("**".into())];
        let input = serde_json::json!({});

        let result = execute_tool("file_write", &input, &caps, &context)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("missing 'path'"));
    }

    #[tokio::test]
    async fn test_dispatch_file_write_missing_content() {
        let context = make_test_context(None);
        let caps = vec![Capability::FileWrite("**".into())];
        let input = serde_json::json!({"path": "/tmp/test.txt"});

        let result = execute_tool("file_write", &input, &caps, &context)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("missing 'content'"));
    }

    // -----------------------------------------------------------------------
    // SSRF protection tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_private_ip_link_local() {
        assert!(is_private_ip(&"169.254.1.1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_broadcast() {
        assert!(is_private_ip(&"255.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_unspecified() {
        assert!(is_private_ip(&"0.0.0.0".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_loopback() {
        assert!(is_private_ip(&"::1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_unspecified() {
        assert!(is_private_ip(&"::".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_172_16_range() {
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(is_private_ip(&"172.31.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_not_private_public_ips() {
        assert!(!is_private_ip(&"8.8.4.4".parse().unwrap()));
        assert!(!is_private_ip(&"142.250.80.46".parse().unwrap()));
        assert!(!is_private_ip(&"104.16.132.229".parse().unwrap()));
    }

    // -----------------------------------------------------------------------
    // JSON path query tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_json_path_query_nested() {
        let data = serde_json::json!({"a": {"b": {"c": 42}}});
        assert_eq!(json_path_query(&data, "a.b.c"), serde_json::json!(42));
    }

    #[test]
    fn test_json_path_query_array_index() {
        let data = serde_json::json!({"items": [10, 20, 30]});
        assert_eq!(json_path_query(&data, "items.2"), serde_json::json!(30));
    }

    #[test]
    fn test_json_path_query_missing_key() {
        let data = serde_json::json!({"a": 1});
        assert_eq!(json_path_query(&data, "b"), serde_json::json!(null));
    }

    #[test]
    fn test_json_path_query_empty_path() {
        let data = serde_json::json!({"a": 1});
        assert_eq!(json_path_query(&data, ""), data);
    }

    #[test]
    fn test_json_path_query_deeply_nested() {
        let data = serde_json::json!({"l1": {"l2": {"l3": {"l4": "deep"}}}});
        assert_eq!(
            json_path_query(&data, "l1.l2.l3.l4"),
            serde_json::json!("deep")
        );
    }

    #[test]
    fn test_json_path_query_array_of_objects() {
        let data = serde_json::json!({"users": [{"name": "Alice"}, {"name": "Bob"}]});
        assert_eq!(
            json_path_query(&data, "users.0.name"),
            serde_json::json!("Alice")
        );
    }

    // -----------------------------------------------------------------------
    // resolve_path tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_path_absolute() {
        let result = resolve_path(std::path::Path::new("/tmp"), "/etc/hosts").unwrap();
        assert_eq!(result, std::path::PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let result = resolve_path(std::path::Path::new("/home/user"), "file.txt").unwrap();
        assert_eq!(result, std::path::PathBuf::from("/home/user/file.txt"));
    }

    #[test]
    fn test_resolve_path_dot_prefix() {
        let result = resolve_path(std::path::Path::new("/work"), "./src/lib.rs").unwrap();
        assert_eq!(result, std::path::PathBuf::from("/work/./src/lib.rs"));
    }

    // -----------------------------------------------------------------------
    // compute_hash tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compute_hash_sha256() {
        let hash = compute_hash("sha256", b"test").unwrap();
        assert_eq!(
            hash,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }

    #[test]
    fn test_compute_hash_sha512() {
        let hash = compute_hash("sha512", b"test").unwrap();
        // SHA-512 of "test" is known
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 128); // SHA-512 produces 128 hex chars
    }

    #[test]
    fn test_compute_hash_md5_rejected() {
        let result = compute_hash("md5", b"test");
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_hash_unknown_algo() {
        let result = compute_hash("blake2", b"test");
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_hash_sha256_empty() {
        let hash = compute_hash("sha256", b"").unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // -----------------------------------------------------------------------
    // strip_html_tags tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_strip_html_nested_tags() {
        assert_eq!(strip_html_tags("<div><span>text</span></div>"), "text");
    }

    #[test]
    fn test_strip_html_empty() {
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn test_strip_html_no_tags() {
        assert_eq!(strip_html_tags("plain text"), "plain text");
    }

    // -----------------------------------------------------------------------
    // Additional regex and data manipulation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_regex_match_no_match() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "pattern": r"\d+",
            "text": "no numbers here",
            "global": false
        });

        let result = execute_tool("regex_match", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["matched"], false);
    }

    #[tokio::test]
    async fn test_regex_match_global() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "pattern": r"\d+",
            "text": "abc 123 def 456 ghi 789",
            "global": true
        });

        let result = execute_tool("regex_match", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 3);
    }

    #[tokio::test]
    async fn test_regex_match_invalid_pattern() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "pattern": r"[invalid",
            "text": "test"
        });

        let result = execute_tool("regex_match", &input, &caps, &context)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid regex"));
    }

    #[tokio::test]
    async fn test_json_query_string_data() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "data": r#"{"key": "value"}"#,
            "path": "key"
        });

        let result = execute_tool("json_query", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output, serde_json::json!("value"));
    }

    #[tokio::test]
    async fn test_json_transform_filter() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "data": [
                {"name": "Alice", "role": "admin"},
                {"name": "Bob", "role": "user"},
                {"name": "Carol", "role": "admin"}
            ],
            "filter_key": "role",
            "filter_value": "admin"
        });

        let result = execute_tool("json_transform", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        let arr = result.output.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[tokio::test]
    async fn test_yaml_parse_nested_mapping() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "content": "server:\n  host: localhost\n  port: 8080"
        });

        let result = execute_tool("yaml_parse", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["server"]["host"], "localhost");
        assert_eq!(result.output["server"]["port"], 8080);
    }

    #[tokio::test]
    async fn test_yaml_parse_invalid() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "content": ":\n  - invalid:\nyaml: [{"
        });

        let result = execute_tool("yaml_parse", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("parse YAML"));
    }

    // -----------------------------------------------------------------------
    // Template render edge cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_template_render_missing_variable() {
        let context = make_test_context(None);
        let caps = vec![Capability::Template];

        let input = serde_json::json!({
            "template": "Hello, {{name}}! Age: {{age}}",
            "variables": {"name": "Alice"}
        });

        let result = execute_tool("template_render", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        let rendered = result.output.as_str().unwrap();
        assert!(rendered.contains("Alice"));
        // Missing variable should stay as placeholder
        assert!(rendered.contains("{{age}}"));
    }

    #[tokio::test]
    async fn test_template_render_no_variables_in_template() {
        let context = make_test_context(None);
        let caps = vec![Capability::Template];

        let input = serde_json::json!({
            "template": "No variables here",
            "variables": {}
        });

        let result = execute_tool("template_render", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output, "No variables here");
    }

    // -----------------------------------------------------------------------
    // Hash tools edge cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_hash_compute_sha512() {
        let context = make_test_context(None);
        let caps = vec![Capability::Crypto];

        let input = serde_json::json!({
            "algorithm": "sha512",
            "input": "test"
        });

        let result = execute_tool("hash_compute", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["algorithm"], "sha512");
        let hash = result.output["hash"].as_str().unwrap();
        assert_eq!(hash.len(), 128);
    }

    #[tokio::test]
    async fn test_hash_compute_no_input_or_file() {
        let context = make_test_context(None);
        let caps = vec![Capability::Crypto];

        let input = serde_json::json!({"algorithm": "sha256"});

        let result = execute_tool("hash_compute", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("must provide"));
    }

    #[tokio::test]
    async fn test_hash_verify_mismatch() {
        let context = make_test_context(None);
        let caps = vec![Capability::Crypto];

        let input = serde_json::json!({
            "algorithm": "sha256",
            "input": "hello",
            "expected": "0000000000000000000000000000000000000000000000000000000000000000"
        });

        let result = execute_tool("hash_verify", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["matches"], false);
    }

    // -----------------------------------------------------------------------
    // Text tool edge cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_text_count_empty() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({"text": ""});

        let result = execute_tool("text_count", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["lines"], 0);
        assert_eq!(result.output["words"], 0);
        assert_eq!(result.output["characters"], 0);
        assert_eq!(result.output["bytes"], 0);
    }

    #[tokio::test]
    async fn test_text_diff_identical() {
        let context = make_test_context(None);
        let caps = vec![Capability::DataManipulation];

        let input = serde_json::json!({
            "old_text": "same text",
            "new_text": "same text"
        });

        let result = execute_tool("text_diff", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["has_changes"], false);
    }

    // -----------------------------------------------------------------------
    // Env tools
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_env_get_nonexistent_var() {
        let context = make_test_context(None);
        let caps = vec![Capability::ShellExec("*".to_string())];

        let input = serde_json::json!({"name": "PUNCH_NONEXISTENT_VAR_12345"});

        let result = execute_tool("env_get", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output["value"].is_null());
    }

    #[tokio::test]
    async fn test_env_list_with_prefix() {
        let context = make_test_context(None);
        let caps = vec![Capability::ShellExec("*".to_string())];

        let input = serde_json::json!({"prefix": "PATH"});

        let result = execute_tool("env_list", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        // PATH should be in the results
        let count = result.output["count"].as_u64().unwrap();
        assert!(count >= 1);
    }

    // -----------------------------------------------------------------------
    // Capability edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_require_capability_multiple_grants() {
        let caps = vec![
            Capability::FileRead("src/**".into()),
            Capability::FileRead("tests/**".into()),
        ];
        assert!(require_capability(&caps, &Capability::FileRead("src/main.rs".into())).is_ok());
        assert!(require_capability(&caps, &Capability::FileRead("tests/test.rs".into())).is_ok());
    }

    #[test]
    fn test_require_capability_empty_caps() {
        let caps: Vec<Capability> = vec![];
        assert!(require_capability(&caps, &Capability::Memory).is_err());
    }

    #[test]
    fn test_require_capability_wrong_type() {
        let caps = vec![Capability::FileRead("**".into())];
        assert!(require_capability(&caps, &Capability::FileWrite("test.txt".into())).is_err());
    }

    // -----------------------------------------------------------------------
    // File read/write round-trip
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_file_write_and_read_roundtrip() {
        let context = make_test_context(None);
        let temp_file = context.working_dir.join("punch_roundtrip_test.txt");
        let caps = vec![
            Capability::FileRead("**".into()),
            Capability::FileWrite("**".into()),
        ];

        // Write
        let write_input = serde_json::json!({
            "path": temp_file.to_string_lossy(),
            "content": "roundtrip content"
        });
        let write_result = execute_tool("file_write", &write_input, &caps, &context)
            .await
            .unwrap();
        assert!(write_result.success);

        // Read back
        let read_input = serde_json::json!({
            "path": temp_file.to_string_lossy()
        });
        let read_result = execute_tool("file_read", &read_input, &caps, &context)
            .await
            .unwrap();
        assert!(read_result.success);
        assert_eq!(read_result.output, "roundtrip content");

        let _ = tokio::fs::remove_file(&temp_file).await;
    }

    // -----------------------------------------------------------------------
    // File list test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_file_list_temp_dir() {
        let context = make_test_context(None);
        let caps = vec![Capability::FileRead("**".into())];

        let input = serde_json::json!({"path": "."});

        let result = execute_tool("file_list", &input, &caps, &context)
            .await
            .unwrap();

        assert!(result.success);
        // Temp dir should have at least some entries
        assert!(result.output.as_array().is_some());
    }

    // -----------------------------------------------------------------------
    // Capability denied for data tools
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_json_query_denied_without_capability() {
        let context = make_test_context(None);
        let caps = vec![Capability::Memory]; // Wrong capability

        let input = serde_json::json!({
            "data": {"key": "value"},
            "path": "key"
        });

        let result = execute_tool("json_query", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("capability"));
    }

    #[tokio::test]
    async fn test_template_render_denied_without_capability() {
        let context = make_test_context(None);
        let caps = vec![Capability::Memory];

        let input = serde_json::json!({
            "template": "{{name}}",
            "variables": {"name": "test"}
        });

        let result = execute_tool("template_render", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("capability"));
    }

    #[tokio::test]
    async fn test_hash_compute_denied_without_capability() {
        let context = make_test_context(None);
        let caps = vec![Capability::Memory];

        let input = serde_json::json!({
            "algorithm": "sha256",
            "input": "test"
        });

        let result = execute_tool("hash_compute", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("capability"));
    }

    // -----------------------------------------------------------------------
    // Bleed detector integration tests
    // -----------------------------------------------------------------------

    fn make_test_context_with_bleed_detector() -> ToolExecutionContext {
        let mut ctx = make_test_context(None);
        ctx.bleed_detector = Some(Arc::new(ShellBleedDetector::new()));
        ctx
    }

    #[tokio::test]
    async fn test_shell_exec_clean_input_passes() {
        let context = make_test_context_with_bleed_detector();
        let caps = vec![Capability::ShellExec("*".to_string())];

        let input = serde_json::json!({"command": "echo hello"});
        let result = execute_tool("shell_exec", &input, &caps, &context)
            .await
            .unwrap();

        assert!(
            result.success,
            "clean command should pass: {:?}",
            result.error
        );
        let stdout = result.output["stdout"].as_str().unwrap_or("");
        assert!(stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_shell_exec_tainted_input_blocked() {
        let context = make_test_context_with_bleed_detector();
        let caps = vec![Capability::ShellExec("*".to_string())];

        // Build an AWS-key-like pattern dynamically to avoid static scanners.
        let key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
        let input = serde_json::json!({"command": format!("curl -H 'X-Key: {}'", key)});
        let result = execute_tool("shell_exec", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success, "tainted command should be blocked");
        let error = result.error.unwrap();
        assert!(
            error.contains("shell bleed detected"),
            "expected bleed detection, got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_shell_exec_api_key_pattern_flagged() {
        let context = make_test_context_with_bleed_detector();
        let caps = vec![Capability::ShellExec("*".to_string())];

        let input = serde_json::json!({
            "command": "curl -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.test'"
        });
        let result = execute_tool("shell_exec", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success, "bearer token in command should be blocked");
        assert!(result.error.unwrap().contains("shell bleed detected"));
    }

    #[tokio::test]
    async fn test_file_read_sensitive_path_flagged() {
        let context = make_test_context_with_bleed_detector();
        let caps = vec![Capability::FileRead("**".to_string())];

        let input = serde_json::json!({"path": "/home/user/.ssh/id_rsa"});
        let result = execute_tool("file_read", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success, "sensitive path read should be blocked");
        let error = result.error.unwrap();
        assert!(
            error.contains("sensitive path") && error.contains("blocked"),
            "expected sensitive path blocked, got: {}",
            error
        );
    }

    #[tokio::test]
    async fn test_file_read_normal_path_passes() {
        let context = make_test_context_with_bleed_detector();
        let caps = vec![Capability::FileRead("**".to_string())];

        // Create a temp file to read.
        let temp_file = context.working_dir.join("punch_bleed_test_normal.txt");
        tokio::fs::write(&temp_file, "normal content")
            .await
            .expect("write temp file");

        let input = serde_json::json!({"path": temp_file.to_string_lossy()});
        let result = execute_tool("file_read", &input, &caps, &context)
            .await
            .unwrap();

        assert!(
            result.success,
            "normal path should pass: {:?}",
            result.error
        );
        let _ = tokio::fs::remove_file(&temp_file).await;
    }

    #[test]
    fn test_bleed_detector_records_security_events() {
        let detector = ShellBleedDetector::new();

        // Clean command produces no warnings.
        let clean = detector.scan_command("ls -la /tmp");
        assert!(clean.is_empty(), "clean command should produce no warnings");

        // Tainted command produces warnings.
        let key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
        let tainted = detector.scan_command(&format!("export AWS_KEY={}", key));
        assert!(
            !tainted.is_empty(),
            "tainted command should produce warnings"
        );

        // Bearer token produces warnings.
        let bearer =
            detector.scan_command("curl -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.test'");
        assert!(!bearer.is_empty(), "bearer token should produce warnings");
    }

    #[test]
    fn test_is_sensitive_path_detection() {
        assert!(is_sensitive_path("/home/user/.ssh/id_rsa"));
        assert!(is_sensitive_path("/app/.env"));
        assert!(is_sensitive_path("/home/user/.aws/credentials"));
        assert!(is_sensitive_path("/home/user/.kube/config"));
        assert!(is_sensitive_path("secrets.json"));
        assert!(!is_sensitive_path("/home/user/project/src/main.rs"));
        assert!(!is_sensitive_path("/tmp/output.txt"));
    }

    // -----------------------------------------------------------------------
    // wasm_invoke tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_wasm_invoke_no_registry_returns_error() {
        let context = make_test_context(None);
        let caps = vec![Capability::PluginInvoke];
        let input = serde_json::json!({
            "plugin": "test-plugin",
            "function": "execute"
        });

        let result = execute_tool("wasm_invoke", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap()
                .contains("plugin runtime not configured"),
            "expected plugin runtime error, got: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn test_wasm_invoke_missing_capability() {
        let context = make_test_context(None);
        // No PluginInvoke capability
        let caps = vec![Capability::Memory];
        let input = serde_json::json!({
            "plugin": "test-plugin",
            "function": "execute"
        });

        let result = execute_tool("wasm_invoke", &input, &caps, &context).await;
        // Should fail with capability denied
        match result {
            Ok(tr) => assert!(!tr.success),
            Err(e) => assert!(
                e.to_string().contains("capability"),
                "expected capability error, got: {e}"
            ),
        }
    }

    #[tokio::test]
    async fn test_wasm_invoke_missing_plugin_param() {
        use punch_extensions::plugin::{NativePluginRuntime, PluginRegistry};
        let runtime = Arc::new(NativePluginRuntime::new());
        let registry = Arc::new(PluginRegistry::with_runtime(runtime));

        let mut context = make_test_context(None);
        context.plugin_registry = Some(registry);

        let caps = vec![Capability::PluginInvoke];
        let input = serde_json::json!({
            "function": "execute"
        });

        let result = execute_tool("wasm_invoke", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap().contains("plugin"),
            "expected missing plugin param error, got: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn test_wasm_invoke_missing_function_param() {
        use punch_extensions::plugin::{NativePluginRuntime, PluginRegistry};
        let runtime = Arc::new(NativePluginRuntime::new());
        let registry = Arc::new(PluginRegistry::with_runtime(runtime));

        let mut context = make_test_context(None);
        context.plugin_registry = Some(registry);

        let caps = vec![Capability::PluginInvoke];
        let input = serde_json::json!({
            "plugin": "test-plugin"
        });

        let result = execute_tool("wasm_invoke", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap().contains("function"),
            "expected missing function param error, got: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn test_wasm_invoke_plugin_not_found() {
        use punch_extensions::plugin::{NativePluginRuntime, PluginRegistry};
        let runtime = Arc::new(NativePluginRuntime::new());
        let registry = Arc::new(PluginRegistry::with_runtime(runtime));

        let mut context = make_test_context(None);
        context.plugin_registry = Some(registry);

        let caps = vec![Capability::PluginInvoke];
        let input = serde_json::json!({
            "plugin": "nonexistent-plugin",
            "function": "execute"
        });

        let result = execute_tool("wasm_invoke", &input, &caps, &context)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap().contains("not found"),
            "expected plugin not found error, got: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn test_wasm_invoke_success_with_native_runtime() {
        use punch_extensions::plugin::{
            NativePluginRuntime, PluginManifest, PluginOutput, PluginPermissions, PluginRegistry,
        };

        let runtime = Arc::new(NativePluginRuntime::new());
        let registry = Arc::new(PluginRegistry::with_runtime(runtime.clone()));

        let manifest = PluginManifest {
            name: "echo-technique".to_string(),
            version: "1.0.0".to_string(),
            description: "Echoes input back".to_string(),
            author: "Test".to_string(),
            entry_point: "execute".to_string(),
            capabilities: vec![],
            max_memory_bytes: 64 * 1024 * 1024,
            max_execution_ms: 30_000,
            permissions: PluginPermissions::default(),
        };

        let id = registry.register(manifest, b"native").await.unwrap();
        runtime.register_function(id, |input| {
            Ok(PluginOutput {
                result: input.args.clone(),
                logs: vec!["technique executed".to_string()],
                execution_ms: 0,
                memory_used_bytes: 512,
            })
        });

        let mut context = make_test_context(None);
        context.plugin_registry = Some(registry);

        let caps = vec![Capability::PluginInvoke];
        let input = serde_json::json!({
            "plugin": "echo-technique",
            "function": "execute",
            "input": {"strike": "roundhouse"}
        });

        let result = execute_tool("wasm_invoke", &input, &caps, &context)
            .await
            .unwrap();

        assert!(
            result.success,
            "wasm_invoke should succeed: {:?}",
            result.error
        );
        assert_eq!(result.output["result"]["strike"], "roundhouse");
        assert!(!result.output["logs"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_wasm_invoke_default_input() {
        use punch_extensions::plugin::{
            NativePluginRuntime, PluginManifest, PluginOutput, PluginPermissions, PluginRegistry,
        };

        let runtime = Arc::new(NativePluginRuntime::new());
        let registry = Arc::new(PluginRegistry::with_runtime(runtime.clone()));

        let manifest = PluginManifest {
            name: "noop-technique".to_string(),
            version: "1.0.0".to_string(),
            description: "Does nothing".to_string(),
            author: "Test".to_string(),
            entry_point: "execute".to_string(),
            capabilities: vec![],
            max_memory_bytes: 64 * 1024 * 1024,
            max_execution_ms: 30_000,
            permissions: PluginPermissions::default(),
        };

        let id = registry.register(manifest, b"native").await.unwrap();
        runtime.register_function(id, |input| {
            // Verify args default to empty object when "input" is omitted
            assert_eq!(input.args, serde_json::json!({}));
            Ok(PluginOutput {
                result: serde_json::json!("ok"),
                logs: vec![],
                execution_ms: 0,
                memory_used_bytes: 0,
            })
        });

        let mut context = make_test_context(None);
        context.plugin_registry = Some(registry);

        let caps = vec![Capability::PluginInvoke];
        // Omit "input" field — should default to {}
        let input = serde_json::json!({
            "plugin": "noop-technique",
            "function": "execute"
        });

        let result = execute_tool("wasm_invoke", &input, &caps, &context)
            .await
            .unwrap();

        assert!(
            result.success,
            "wasm_invoke should succeed: {:?}",
            result.error
        );
        assert_eq!(result.output["result"], "ok");
    }

    // -----------------------------------------------------------------------
    // A2A delegation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_a2a_delegate_missing_agent_url() {
        let context = make_test_context(None);
        let caps = vec![Capability::A2ADelegate];
        let input = serde_json::json!({"prompt": "hello"});

        let result = execute_tool("a2a_delegate", &input, &caps, &context)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap_or("").contains("agent_url"),
            "error should mention agent_url: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn test_a2a_delegate_missing_prompt() {
        let context = make_test_context(None);
        let caps = vec![Capability::A2ADelegate];
        let input = serde_json::json!({"agent_url": "http://localhost:9999"});

        let result = execute_tool("a2a_delegate", &input, &caps, &context)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap_or("").contains("prompt"),
            "error should mention prompt: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn test_a2a_delegate_capability_denied() {
        let context = make_test_context(None);
        let caps = vec![Capability::Memory]; // no A2ADelegate
        let input = serde_json::json!({
            "agent_url": "http://localhost:9999",
            "prompt": "hello"
        });

        let result = execute_tool("a2a_delegate", &input, &caps, &context).await;
        // Should fail with CapabilityDenied error
        assert!(result.is_err() || !result.unwrap().success);
    }

    #[tokio::test]
    async fn test_a2a_delegate_unreachable_agent() {
        let context = make_test_context(None);
        let caps = vec![Capability::A2ADelegate];
        let input = serde_json::json!({
            "agent_url": "http://127.0.0.1:19999",
            "prompt": "hello",
            "timeout_secs": 2
        });

        let result = execute_tool("a2a_delegate", &input, &caps, &context)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("discovery failed"),
            "error should mention discovery failure: {:?}",
            result.error
        );
    }
}
