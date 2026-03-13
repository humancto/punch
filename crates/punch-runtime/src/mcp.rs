//! MCP (Model Context Protocol) client.
//!
//! Manages connections to MCP servers via stdio transport, providing
//! tool discovery and invocation over JSON-RPC 2.0.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};
use tracing::{debug, info, warn};

use punch_types::{PunchError, PunchResult, ToolCategory, ToolDefinition};

/// A client connection to a single MCP server.
pub struct McpClient {
    /// Name of this MCP server (used for tool namespacing).
    server_name: String,
    /// The child process handle.
    child: Mutex<Option<Child>>,
    /// Sender for writing requests to the child's stdin.
    stdin_tx: Mutex<Option<tokio::process::ChildStdin>>,
    /// Pending requests awaiting responses, keyed by JSON-RPC id.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    /// Monotonic request ID counter.
    next_id: AtomicU64,
    /// Server capabilities discovered during initialization.
    server_info: Mutex<Option<serde_json::Value>>,
}

impl McpClient {
    /// Spawn an MCP server subprocess and prepare the client.
    ///
    /// Does NOT send the `initialize` request yet -- call [`initialize`] after.
    pub async fn spawn(
        server_name: String,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> PunchResult<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| PunchError::Mcp {
            server: server_name.clone(),
            message: format!("failed to spawn: {}", e),
        })?;

        let stdout = child.stdout.take().ok_or_else(|| PunchError::Mcp {
            server: server_name.clone(),
            message: "failed to capture stdout".into(),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| PunchError::Mcp {
            server: server_name.clone(),
            message: "failed to capture stdin".into(),
        })?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn a reader task to route responses to pending requests.
        let pending_clone = Arc::clone(&pending);
        let name_clone = server_name.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(msg) => {
                        if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                            let mut pending = pending_clone.lock().await;
                            if let Some(tx) = pending.remove(&id) {
                                let _ = tx.send(msg);
                            }
                        } else {
                            // Notification from server (no id) -- log and discard.
                            debug!(server = %name_clone, "mcp notification: {}", line);
                        }
                    }
                    Err(e) => {
                        warn!(server = %name_clone, "failed to parse MCP message: {}", e);
                    }
                }
            }

            debug!(server = %name_clone, "MCP stdout reader exited");
        });

        info!(server = %server_name, command = command, "MCP server spawned");

        Ok(Self {
            server_name,
            child: Mutex::new(Some(child)),
            stdin_tx: Mutex::new(Some(stdin)),
            pending,
            next_id: AtomicU64::new(1),
            server_info: Mutex::new(None),
        })
    }

    /// Send the JSON-RPC `initialize` handshake to the MCP server.
    pub async fn initialize(&self) -> PunchResult<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "punch-runtime",
                "version": env!("CARGO_PKG_VERSION"),
            }
        });

        let response = self.send_request("initialize", params).await?;

        // Store the server info for later reference.
        *self.server_info.lock().await = Some(response.clone());

        // Send the `initialized` notification (no id, no response expected).
        self.send_notification("notifications/initialized", serde_json::json!({}))
            .await?;

        info!(server = %self.server_name, "MCP server initialized");
        Ok(())
    }

    /// Discover tools exposed by this MCP server.
    ///
    /// Tool names are namespaced as `mcp_{server_name}_{tool_name}`.
    pub async fn list_tools(&self) -> PunchResult<Vec<ToolDefinition>> {
        let response = self
            .send_request("tools/list", serde_json::json!({}))
            .await?;

        let result = response.get("result").ok_or_else(|| PunchError::Mcp {
            server: self.server_name.clone(),
            message: "missing 'result' in tools/list response".into(),
        })?;

        let tools_array = result
            .get("tools")
            .and_then(|t| t.as_array())
            .ok_or_else(|| PunchError::Mcp {
                server: self.server_name.clone(),
                message: "missing 'tools' array in response".into(),
            })?;

        let mut tools = Vec::new();
        for tool in tools_array {
            let raw_name = tool["name"].as_str().unwrap_or("unknown");
            let namespaced = format!("mcp_{}_{}", self.server_name, raw_name);

            tools.push(ToolDefinition {
                name: namespaced,
                description: tool["description"].as_str().unwrap_or("").to_string(),
                input_schema: tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(serde_json::json!({"type": "object"})),
                category: ToolCategory::Agent,
            });
        }

        debug!(
            server = %self.server_name,
            count = tools.len(),
            "discovered MCP tools"
        );

        Ok(tools)
    }

    /// Call a tool on the MCP server.
    ///
    /// The `name` should be the raw tool name (without the mcp_ prefix).
    pub async fn call_tool(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> PunchResult<serde_json::Value> {
        let params = serde_json::json!({
            "name": name,
            "arguments": input,
        });

        let response = self.send_request("tools/call", params).await?;

        let result = response.get("result").cloned().ok_or_else(|| {
            // Check for error.
            let error_msg = response["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            PunchError::Mcp {
                server: self.server_name.clone(),
                message: format!("tool call '{}' failed: {}", name, error_msg),
            }
        })?;

        Ok(result)
    }

    /// Send a JSON-RPC 2.0 request and wait for the response.
    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> PunchResult<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        self.write_message(&request).await?;

        let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| PunchError::Mcp {
                server: self.server_name.clone(),
                message: format!("timeout waiting for response to '{}'", method),
            })?
            .map_err(|_| PunchError::Mcp {
                server: self.server_name.clone(),
                message: format!("response channel closed for '{}'", method),
            })?;

        // Check for JSON-RPC error.
        if let Some(error) = response.get("error") {
            let code = error["code"].as_i64().unwrap_or(-1);
            let message = error["message"].as_str().unwrap_or("unknown");
            return Err(PunchError::Mcp {
                server: self.server_name.clone(),
                message: format!("JSON-RPC error {}: {}", code, message),
            });
        }

        Ok(response)
    }

    /// Send a JSON-RPC 2.0 notification (no id, no response expected).
    async fn send_notification(&self, method: &str, params: serde_json::Value) -> PunchResult<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        self.write_message(&notification).await
    }

    /// Write a JSON message to the child's stdin, followed by a newline.
    async fn write_message(&self, msg: &serde_json::Value) -> PunchResult<()> {
        let serialized = serde_json::to_string(msg).map_err(|e| PunchError::Mcp {
            server: self.server_name.clone(),
            message: format!("failed to serialize message: {}", e),
        })?;

        let mut stdin_guard = self.stdin_tx.lock().await;
        let stdin = stdin_guard.as_mut().ok_or_else(|| PunchError::Mcp {
            server: self.server_name.clone(),
            message: "stdin not available (server may have exited)".into(),
        })?;

        stdin
            .write_all(serialized.as_bytes())
            .await
            .map_err(|e| PunchError::Mcp {
                server: self.server_name.clone(),
                message: format!("failed to write to stdin: {}", e),
            })?;
        stdin.write_all(b"\n").await.map_err(|e| PunchError::Mcp {
            server: self.server_name.clone(),
            message: format!("failed to write newline: {}", e),
        })?;
        stdin.flush().await.map_err(|e| PunchError::Mcp {
            server: self.server_name.clone(),
            message: format!("failed to flush stdin: {}", e),
        })?;

        Ok(())
    }

    /// Shut down the MCP server process gracefully.
    pub async fn shutdown(&self) -> PunchResult<()> {
        // Drop stdin to signal EOF.
        {
            let mut stdin = self.stdin_tx.lock().await;
            *stdin = None;
        }

        let mut child_guard = self.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                Ok(Ok(status)) => {
                    info!(
                        server = %self.server_name,
                        exit_code = ?status.code(),
                        "MCP server exited"
                    );
                }
                Ok(Err(e)) => {
                    warn!(server = %self.server_name, "error waiting for MCP server: {}", e);
                }
                Err(_) => {
                    warn!(server = %self.server_name, "MCP server did not exit in time, killing");
                    let _ = child.kill().await;
                }
            }
        }

        Ok(())
    }

    /// Extract the raw tool name from a namespaced MCP tool name.
    ///
    /// E.g., `mcp_github_create_issue` with server_name `github` returns `create_issue`.
    pub fn strip_namespace<'a>(&self, namespaced_name: &'a str) -> Option<&'a str> {
        let prefix = format!("mcp_{}_", self.server_name);
        namespaced_name.strip_prefix(&prefix)
    }

    /// The server name used for namespacing.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_namespace_basic() {
        let client = McpClient {
            server_name: "github".to_string(),
            child: Mutex::new(None),
            stdin_tx: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            server_info: Mutex::new(None),
        };

        assert_eq!(
            client.strip_namespace("mcp_github_create_issue"),
            Some("create_issue")
        );
    }

    #[test]
    fn test_strip_namespace_no_match() {
        let client = McpClient {
            server_name: "github".to_string(),
            child: Mutex::new(None),
            stdin_tx: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            server_info: Mutex::new(None),
        };

        assert_eq!(client.strip_namespace("mcp_slack_send"), None);
    }

    #[test]
    fn test_strip_namespace_exact_prefix() {
        let client = McpClient {
            server_name: "fs".to_string(),
            child: Mutex::new(None),
            stdin_tx: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            server_info: Mutex::new(None),
        };

        assert_eq!(
            client.strip_namespace("mcp_fs_read_file"),
            Some("read_file")
        );
        assert_eq!(client.strip_namespace("mcp_fs_"), Some(""));
    }

    #[test]
    fn test_server_name() {
        let client = McpClient {
            server_name: "test-server".to_string(),
            child: Mutex::new(None),
            stdin_tx: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            server_info: Mutex::new(None),
        };

        assert_eq!(client.server_name(), "test-server");
    }

    #[test]
    fn test_next_id_atomic() {
        let client = McpClient {
            server_name: "test".to_string(),
            child: Mutex::new(None),
            stdin_tx: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            server_info: Mutex::new(None),
        };

        let id1 = client.next_id.fetch_add(1, Ordering::Relaxed);
        let id2 = client.next_id.fetch_add(1, Ordering::Relaxed);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }
}
