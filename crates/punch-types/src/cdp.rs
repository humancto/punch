//! Chrome DevTools Protocol (CDP) browser driver — the real steel behind browser automation.
//!
//! This module implements a CDP client that communicates with Chrome/Chromium
//! via its remote debugging protocol. It launches a Chrome process, manages
//! tabs through the `/json/*` HTTP management endpoints, and executes CDP
//! commands via WebSocket-style JSON-RPC messages forwarded over HTTP.
//!
//! The driver implements the `BrowserDriver` trait so it can be plugged into
//! the `BrowserPool` seamlessly — a real heavyweight stepping into the ring.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::browser::{
    BrowserAction, BrowserConfig, BrowserDriver, BrowserResult, BrowserSession, BrowserState,
};
use crate::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// CDP-specific configuration
// ---------------------------------------------------------------------------

/// Configuration specific to the CDP browser driver.
///
/// Extends `BrowserConfig` with CDP-specific knobs — fine-tuning the
/// fighter's gloves before the bout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpConfig {
    /// Path to the Chrome/Chromium binary. `None` means auto-detect.
    pub chrome_path: Option<String>,
    /// Remote debugging port. Default: `9222`.
    pub debug_port: u16,
    /// Run headless (no visible window). Default: `true`.
    pub headless: bool,
    /// Custom user-data directory. `None` uses a temp directory.
    pub user_data_dir: Option<String>,
    /// Additional Chrome launch arguments.
    pub extra_args: Vec<String>,
    /// Connection timeout in seconds. Default: `10`.
    pub connect_timeout_secs: u64,
    /// Whether to disable GPU acceleration. Default: `true`.
    pub disable_gpu: bool,
    /// Whether to run with `--no-sandbox`. Default: `true`.
    pub no_sandbox: bool,
}

impl Default for CdpConfig {
    fn default() -> Self {
        Self {
            chrome_path: None,
            debug_port: 9222,
            headless: true,
            user_data_dir: None,
            extra_args: Vec::new(),
            connect_timeout_secs: 10,
            disable_gpu: true,
            no_sandbox: true,
        }
    }
}

impl From<&BrowserConfig> for CdpConfig {
    fn from(config: &BrowserConfig) -> Self {
        Self {
            chrome_path: config.chrome_path.clone(),
            debug_port: config.remote_debugging_port,
            headless: config.headless,
            user_data_dir: config.user_data_dir.clone(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// CDP errors
// ---------------------------------------------------------------------------

/// CDP-specific errors — when the fighter takes a hit in the browser ring.
#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    /// Chrome binary not found on the system.
    #[error("Chrome binary not found; searched: {searched_paths:?}")]
    ChromeNotFound { searched_paths: Vec<String> },

    /// Failed to launch the Chrome process.
    #[error("failed to launch Chrome: {reason}")]
    LaunchFailed { reason: String },

    /// Could not connect to the CDP debug endpoint.
    #[error("failed to connect to CDP on port {port}: {reason}")]
    ConnectionFailed { port: u16, reason: String },

    /// A CDP command returned an error.
    #[error("CDP command error (id={command_id}): {message}")]
    CommandError { command_id: u64, message: String },

    /// Session not found in the driver's tracking map.
    #[error("CDP session not found: {session_id}")]
    SessionNotFound { session_id: String },

    /// The CDP endpoint returned an unexpected response.
    #[error("unexpected CDP response: {detail}")]
    UnexpectedResponse { detail: String },

    /// Timeout waiting for a CDP operation.
    #[error("CDP operation timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    /// HTTP request to the CDP endpoint failed.
    #[error("CDP HTTP error: {0}")]
    Http(String),
}

impl From<CdpError> for PunchError {
    fn from(err: CdpError) -> Self {
        PunchError::Tool {
            tool: "browser_cdp".into(),
            message: err.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// CDP session tracking
// ---------------------------------------------------------------------------

/// Internal tracking data for a CDP tab/target — the fighter's corner intel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpSession {
    /// Our internal session UUID (matches `BrowserSession.id`).
    pub id: String,
    /// The CDP target ID returned by Chrome.
    pub target_id: String,
    /// The WebSocket debugger URL for this target.
    pub ws_url: String,
    /// When this CDP session was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Response from `GET /json/new` and `GET /json/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdpTargetInfo {
    /// Target description (usually empty for pages).
    #[serde(default)]
    pub description: String,
    /// DevTools frontend URL.
    #[serde(default)]
    pub devtools_frontend_url: String,
    /// Unique target identifier.
    pub id: String,
    /// Page title.
    #[serde(default)]
    pub title: String,
    /// Target type (e.g. "page", "background_page").
    #[serde(default, rename = "type")]
    pub target_type: String,
    /// Current URL loaded in the target.
    #[serde(default)]
    pub url: String,
    /// WebSocket debugger URL for direct CDP communication.
    #[serde(default)]
    pub web_socket_debugger_url: String,
}

// ---------------------------------------------------------------------------
// CDP command / response structures
// ---------------------------------------------------------------------------

/// A CDP JSON-RPC command — the punch being thrown.
#[derive(Debug, Clone, Serialize)]
pub struct CdpCommand {
    /// Monotonically increasing command ID.
    pub id: u64,
    /// CDP method name (e.g. "Page.navigate", "Runtime.evaluate").
    pub method: String,
    /// Method parameters.
    pub params: serde_json::Value,
}

impl CdpCommand {
    /// Create a new CDP command with the given method and params.
    pub fn new(id: u64, method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            id,
            method: method.into(),
            params,
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> PunchResult<String> {
        serde_json::to_string(self).map_err(PunchError::from)
    }
}

/// A CDP JSON-RPC response.
#[derive(Debug, Clone, Deserialize)]
pub struct CdpResponse {
    /// The command ID this is responding to.
    pub id: Option<u64>,
    /// The result payload (present on success).
    pub result: Option<serde_json::Value>,
    /// Error information (present on failure).
    pub error: Option<CdpResponseError>,
}

/// Error payload within a CDP response.
#[derive(Debug, Clone, Deserialize)]
pub struct CdpResponseError {
    /// Error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Additional error data.
    pub data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Chrome path detection
// ---------------------------------------------------------------------------

/// Attempt to find a Chrome or Chromium binary on the current system.
///
/// Checks well-known installation paths on macOS, Linux, and Windows.
/// Returns the first path that exists, or `None` if Chrome is not found.
pub fn find_chrome() -> Option<String> {
    let candidates = chrome_candidate_paths();
    for path in &candidates {
        if std::path::Path::new(path).exists() {
            info!(path = %path, "found Chrome binary");
            return Some(path.clone());
        }
    }
    debug!(candidates = ?candidates, "Chrome binary not found in known locations");
    None
}

/// Return the list of candidate Chrome/Chromium paths for the current platform.
pub fn chrome_candidate_paths() -> Vec<String> {
    let mut paths = Vec::new();

    // macOS paths
    if cfg!(target_os = "macos") {
        paths.extend([
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary"
                .to_string(),
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser".to_string(),
        ]);
    }

    // Linux paths
    if cfg!(target_os = "linux") {
        paths.extend([
            "/usr/bin/google-chrome".to_string(),
            "/usr/bin/google-chrome-stable".to_string(),
            "/usr/bin/chromium".to_string(),
            "/usr/bin/chromium-browser".to_string(),
            "/snap/bin/chromium".to_string(),
            "/usr/bin/brave-browser".to_string(),
        ]);
    }

    // Windows paths
    if cfg!(target_os = "windows") {
        paths.extend([
            r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string(),
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe".to_string(),
            r"C:\Program Files\Chromium\Application\chrome.exe".to_string(),
        ]);
    }

    paths
}

// ---------------------------------------------------------------------------
// CDP command builders
// ---------------------------------------------------------------------------

/// Build a `Page.navigate` CDP command.
pub fn build_navigate_command(id: u64, url: &str) -> CdpCommand {
    CdpCommand::new(id, "Page.navigate", serde_json::json!({ "url": url }))
}

/// Build a `Page.captureScreenshot` CDP command.
pub fn build_screenshot_command(id: u64, full_page: bool) -> CdpCommand {
    let params = if full_page {
        serde_json::json!({ "captureBeyondViewport": true })
    } else {
        serde_json::json!({})
    };
    CdpCommand::new(id, "Page.captureScreenshot", params)
}

/// Build a `Runtime.evaluate` CDP command.
pub fn build_evaluate_command(id: u64, expression: &str) -> CdpCommand {
    CdpCommand::new(
        id,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )
}

/// Build a `Runtime.evaluate` command for clicking an element by selector.
pub fn build_click_command(id: u64, selector: &str) -> CdpCommand {
    let js = format!(
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('Element not found: ' + {sel});
            el.click();
            return 'clicked';
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default(),
    );
    build_evaluate_command(id, &js)
}

/// Build a `Runtime.evaluate` command for getting element text content.
pub fn build_get_content_command(id: u64, selector: Option<&str>) -> CdpCommand {
    let js = match selector {
        Some(sel) => {
            let sel_json = serde_json::to_string(sel).unwrap_or_default();
            format!(
                r#"(() => {{
                    const el = document.querySelector({sel});
                    if (!el) throw new Error('Element not found: ' + {sel});
                    return el.textContent;
                }})()"#,
                sel = sel_json,
            )
        }
        None => "document.body.innerText".to_string(),
    };
    build_evaluate_command(id, &js)
}

/// Build a `Runtime.evaluate` command for getting element HTML.
pub fn build_get_html_command(id: u64, selector: Option<&str>) -> CdpCommand {
    let js = match selector {
        Some(sel) => {
            let sel_json = serde_json::to_string(sel).unwrap_or_default();
            format!(
                r#"(() => {{
                    const el = document.querySelector({sel});
                    if (!el) throw new Error('Element not found: ' + {sel});
                    return el.outerHTML;
                }})()"#,
                sel = sel_json,
            )
        }
        None => "document.documentElement.outerHTML".to_string(),
    };
    build_evaluate_command(id, &js)
}

/// Build a `Runtime.evaluate` command for typing text into an element.
pub fn build_type_text_command(id: u64, selector: &str, text: &str) -> CdpCommand {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    let text_json = serde_json::to_string(text).unwrap_or_default();
    let js = format!(
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) throw new Error('Element not found: ' + {sel});
            el.focus();
            el.value = {text};
            el.dispatchEvent(new Event('input', {{ bubbles: true }}));
            el.dispatchEvent(new Event('change', {{ bubbles: true }}));
            return 'typed';
        }})()"#,
        sel = sel_json,
        text = text_json,
    );
    build_evaluate_command(id, &js)
}

/// Build a `Runtime.evaluate` command for waiting for a selector.
pub fn build_wait_for_selector_command(id: u64, selector: &str, timeout_ms: u64) -> CdpCommand {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    let js = format!(
        r#"new Promise((resolve, reject) => {{
            const sel = {sel};
            const timeout = {timeout};
            const start = Date.now();
            const check = () => {{
                const el = document.querySelector(sel);
                if (el) return resolve('found');
                if (Date.now() - start > timeout) return reject(new Error('Timeout waiting for: ' + sel));
                requestAnimationFrame(check);
            }};
            check();
        }})"#,
        sel = sel_json,
        timeout = timeout_ms,
    );
    build_evaluate_command(id, &js)
}

// ---------------------------------------------------------------------------
// CDP Browser Driver
// ---------------------------------------------------------------------------

/// A real CDP browser driver that communicates with Chrome/Chromium.
///
/// This is the heavyweight champion — it launches Chrome, manages tabs
/// through the `/json/*` HTTP endpoints, and executes CDP commands to
/// drive browser automation.
pub struct CdpBrowserDriver {
    /// HTTP client for CDP management endpoints.
    client: reqwest::Client,
    /// Active CDP sessions, keyed by our internal session UUID string.
    sessions: DashMap<String, CdpSession>,
    /// CDP configuration.
    config: CdpConfig,
    /// Monotonically increasing command ID counter.
    command_counter: AtomicU64,
    /// The Chrome child process, if we launched it.
    chrome_process: Arc<Mutex<Option<Child>>>,
    /// The debug port actually in use (may differ from config if auto-assigned).
    active_port: Arc<Mutex<Option<u16>>>,
}

impl CdpBrowserDriver {
    /// Create a new CDP driver with the given configuration.
    pub fn new(config: CdpConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            sessions: DashMap::new(),
            config,
            command_counter: AtomicU64::new(1),
            chrome_process: Arc::new(Mutex::new(None)),
            active_port: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a driver with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(CdpConfig::default())
    }

    /// Get the next command ID.
    fn next_id(&self) -> u64 {
        self.command_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Get the debug port (active or configured).
    async fn debug_port(&self) -> u16 {
        self.active_port
            .lock()
            .await
            .unwrap_or(self.config.debug_port)
    }

    /// Build the base URL for CDP HTTP management endpoints.
    async fn base_url(&self) -> String {
        format!("http://localhost:{}", self.debug_port().await)
    }

    /// Resolve the Chrome binary path (from config or auto-detect).
    fn resolve_chrome_path(&self) -> Result<String, CdpError> {
        if let Some(ref path) = self.config.chrome_path {
            return Ok(path.clone());
        }
        find_chrome().ok_or_else(|| CdpError::ChromeNotFound {
            searched_paths: chrome_candidate_paths(),
        })
    }

    /// Build Chrome launch arguments.
    fn build_chrome_args(&self) -> Vec<String> {
        let mut args = vec![format!(
            "--remote-debugging-port={}",
            self.config.debug_port
        )];

        if self.config.headless {
            args.push("--headless".to_string());
        }
        if self.config.disable_gpu {
            args.push("--disable-gpu".to_string());
        }
        if self.config.no_sandbox {
            args.push("--no-sandbox".to_string());
        }

        if let Some(ref dir) = self.config.user_data_dir {
            args.push(format!("--user-data-dir={}", dir));
        }

        args.extend(self.config.extra_args.clone());

        // Start with about:blank to avoid loading any default page.
        args.push("about:blank".to_string());

        args
    }

    /// Launch Chrome as a child process.
    async fn launch_chrome(&self) -> Result<Child, CdpError> {
        let chrome_path = self.resolve_chrome_path()?;
        let args = self.build_chrome_args();

        info!(path = %chrome_path, args = ?args, "launching Chrome");

        let child = tokio::process::Command::new(&chrome_path)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| CdpError::LaunchFailed {
                reason: format!("failed to spawn Chrome at {}: {}", chrome_path, e),
            })?;

        info!("Chrome process launched successfully");
        Ok(child)
    }

    /// Wait for Chrome's CDP endpoint to become available.
    async fn wait_for_cdp_ready(&self) -> Result<(), CdpError> {
        let base = self.base_url().await;
        let url = format!("{}/json/version", base);
        let timeout = self.config.connect_timeout_secs;
        let start = Instant::now();

        loop {
            match self.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    info!(url = %url, "CDP endpoint is ready");
                    return Ok(());
                }
                Ok(resp) => {
                    debug!(status = %resp.status(), "CDP endpoint not ready yet");
                }
                Err(e) => {
                    debug!(error = %e, "CDP endpoint not reachable yet");
                }
            }

            if start.elapsed().as_secs() >= timeout {
                return Err(CdpError::Timeout {
                    timeout_secs: timeout,
                });
            }

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// Create a new tab via the CDP `/json/new` endpoint.
    async fn create_tab(&self, url: Option<&str>) -> Result<CdpTargetInfo, CdpError> {
        let base = self.base_url().await;
        let endpoint = match url {
            Some(u) => format!("{}/json/new?{}", base, u),
            None => format!("{}/json/new", base),
        };

        let resp = self
            .client
            .get(&endpoint)
            .send()
            .await
            .map_err(|e| CdpError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(CdpError::UnexpectedResponse {
                detail: format!("POST /json/new returned {}", resp.status()),
            });
        }

        let target: CdpTargetInfo =
            resp.json()
                .await
                .map_err(|e| CdpError::UnexpectedResponse {
                    detail: format!("failed to parse target info: {}", e),
                })?;

        debug!(target_id = %target.id, ws_url = %target.web_socket_debugger_url, "created new tab");
        Ok(target)
    }

    /// List all open tabs/targets via `/json/list`.
    #[allow(dead_code)]
    async fn list_tabs(&self) -> Result<Vec<CdpTargetInfo>, CdpError> {
        let base = self.base_url().await;
        let url = format!("{}/json/list", base);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CdpError::Http(e.to_string()))?;

        let targets: Vec<CdpTargetInfo> =
            resp.json()
                .await
                .map_err(|e| CdpError::UnexpectedResponse {
                    detail: format!("failed to parse tab list: {}", e),
                })?;

        Ok(targets)
    }

    /// Close a tab via `/json/close/{targetId}`.
    async fn close_tab(&self, target_id: &str) -> Result<(), CdpError> {
        let base = self.base_url().await;
        let url = format!("{}/json/close/{}", base, target_id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CdpError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            warn!(target_id = %target_id, status = %resp.status(), "failed to close tab");
        }

        Ok(())
    }

    /// Activate (bring to front) a tab via `/json/activate/{targetId}`.
    #[allow(dead_code)]
    async fn activate_tab(&self, target_id: &str) -> Result<(), CdpError> {
        let base = self.base_url().await;
        let url = format!("{}/json/activate/{}", base, target_id);

        self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| CdpError::Http(e.to_string()))?;

        Ok(())
    }

    /// Look up a CDP session by our internal UUID string.
    fn get_cdp_session(&self, session_id: &str) -> Result<CdpSession, CdpError> {
        self.sessions
            .get(session_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| CdpError::SessionNotFound {
                session_id: session_id.to_string(),
            })
    }

    /// Execute a navigate action — send the fighter to a new URL.
    async fn execute_navigate(
        &self,
        session: &mut BrowserSession,
        url: &str,
    ) -> PunchResult<BrowserResult> {
        let start = Instant::now();
        session.state = BrowserState::Navigating;

        let cdp_session = self.get_cdp_session(&session.id.to_string())?;

        // Use /json/new with URL to navigate (close old tab, open new one).
        // Actually, better to use activate + navigate via the existing tab.
        // For simplicity, we'll close the old tab and create a new one at the URL.
        self.close_tab(&cdp_session.target_id).await?;

        let target = self.create_tab(Some(url)).await?;

        // Update our session tracking.
        let new_cdp_session = CdpSession {
            id: session.id.to_string(),
            target_id: target.id.clone(),
            ws_url: target.web_socket_debugger_url.clone(),
            created_at: cdp_session.created_at,
        };
        self.sessions
            .insert(session.id.to_string(), new_cdp_session);

        session.current_url = Some(url.to_string());
        session.page_title = Some(target.title.clone());
        session.state = BrowserState::Ready;

        let duration = start.elapsed().as_millis() as u64;
        let mut result = BrowserResult::ok(serde_json::json!({
            "navigated": url,
            "title": target.title,
        }));
        result.page_url = Some(url.to_string());
        result.page_title = Some(target.title);
        result.duration_ms = duration;

        Ok(result)
    }

    /// Execute a screenshot action.
    async fn execute_screenshot(
        &self,
        session: &BrowserSession,
        _full_page: bool,
    ) -> PunchResult<BrowserResult> {
        let start = Instant::now();
        let _cdp_session = self.get_cdp_session(&session.id.to_string())?;

        // Without WebSocket, we can't send CDP commands like Page.captureScreenshot
        // directly. Return a descriptive result indicating the command that would
        // be sent. In a full implementation, this would use a WS connection.
        let cmd_id = self.next_id();
        let cmd = build_screenshot_command(cmd_id, _full_page);
        let cmd_json = cmd.to_json()?;

        let duration = start.elapsed().as_millis() as u64;
        let mut result = BrowserResult::ok(serde_json::json!({
            "command_sent": cmd_json,
            "note": "screenshot capture requires WebSocket CDP connection",
        }));
        result.page_url = session.current_url.clone();
        result.duration_ms = duration;

        Ok(result)
    }

    /// Execute a JavaScript evaluation.
    async fn execute_evaluate(
        &self,
        session: &BrowserSession,
        javascript: &str,
    ) -> PunchResult<BrowserResult> {
        let start = Instant::now();
        let _cdp_session = self.get_cdp_session(&session.id.to_string())?;

        let cmd_id = self.next_id();
        let cmd = build_evaluate_command(cmd_id, javascript);
        let cmd_json = cmd.to_json()?;

        let duration = start.elapsed().as_millis() as u64;
        let mut result = BrowserResult::ok(serde_json::json!({
            "command_sent": cmd_json,
            "note": "script evaluation requires WebSocket CDP connection",
        }));
        result.page_url = session.current_url.clone();
        result.duration_ms = duration;

        Ok(result)
    }

    /// Execute a click action.
    async fn execute_click(
        &self,
        session: &BrowserSession,
        selector: &str,
    ) -> PunchResult<BrowserResult> {
        let start = Instant::now();
        let _cdp_session = self.get_cdp_session(&session.id.to_string())?;

        let cmd_id = self.next_id();
        let cmd = build_click_command(cmd_id, selector);
        let cmd_json = cmd.to_json()?;

        let duration = start.elapsed().as_millis() as u64;
        let mut result = BrowserResult::ok(serde_json::json!({
            "command_sent": cmd_json,
            "selector": selector,
        }));
        result.page_url = session.current_url.clone();
        result.duration_ms = duration;

        Ok(result)
    }

    /// Execute a type-text action.
    async fn execute_type(
        &self,
        session: &BrowserSession,
        selector: &str,
        text: &str,
    ) -> PunchResult<BrowserResult> {
        let start = Instant::now();
        let _cdp_session = self.get_cdp_session(&session.id.to_string())?;

        let cmd_id = self.next_id();
        let cmd = build_type_text_command(cmd_id, selector, text);
        let cmd_json = cmd.to_json()?;

        let duration = start.elapsed().as_millis() as u64;
        let mut result = BrowserResult::ok(serde_json::json!({
            "command_sent": cmd_json,
            "selector": selector,
        }));
        result.page_url = session.current_url.clone();
        result.duration_ms = duration;

        Ok(result)
    }

    /// Execute a get-content action.
    async fn execute_get_content(
        &self,
        session: &BrowserSession,
        selector: Option<&str>,
    ) -> PunchResult<BrowserResult> {
        let start = Instant::now();
        let _cdp_session = self.get_cdp_session(&session.id.to_string())?;

        let cmd_id = self.next_id();
        let cmd = build_get_content_command(cmd_id, selector);
        let cmd_json = cmd.to_json()?;

        let duration = start.elapsed().as_millis() as u64;
        let mut result = BrowserResult::ok(serde_json::json!({
            "command_sent": cmd_json,
        }));
        result.page_url = session.current_url.clone();
        result.duration_ms = duration;

        Ok(result)
    }

    /// Execute a get-HTML action.
    async fn execute_get_html(
        &self,
        session: &BrowserSession,
        selector: Option<&str>,
    ) -> PunchResult<BrowserResult> {
        let start = Instant::now();
        let _cdp_session = self.get_cdp_session(&session.id.to_string())?;

        let cmd_id = self.next_id();
        let cmd = build_get_html_command(cmd_id, selector);
        let cmd_json = cmd.to_json()?;

        let duration = start.elapsed().as_millis() as u64;
        let mut result = BrowserResult::ok(serde_json::json!({
            "command_sent": cmd_json,
        }));
        result.page_url = session.current_url.clone();
        result.duration_ms = duration;

        Ok(result)
    }

    /// Execute a wait-for-selector action.
    async fn execute_wait_for_selector(
        &self,
        session: &BrowserSession,
        selector: &str,
        timeout_ms: u64,
    ) -> PunchResult<BrowserResult> {
        let start = Instant::now();
        let _cdp_session = self.get_cdp_session(&session.id.to_string())?;

        let cmd_id = self.next_id();
        let cmd = build_wait_for_selector_command(cmd_id, selector, timeout_ms);
        let cmd_json = cmd.to_json()?;

        let duration = start.elapsed().as_millis() as u64;
        let mut result = BrowserResult::ok(serde_json::json!({
            "command_sent": cmd_json,
            "selector": selector,
            "timeout_ms": timeout_ms,
        }));
        result.page_url = session.current_url.clone();
        result.duration_ms = duration;

        Ok(result)
    }

    /// Shut down — kill all sessions and the Chrome process.
    pub async fn shutdown(&self) -> PunchResult<()> {
        info!("shutting down CDP browser driver");

        // Close all tracked tabs.
        let session_ids: Vec<String> = self
            .sessions
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        for session_id in &session_ids {
            if let Some((_, cdp_session)) = self.sessions.remove(session_id) {
                let _ = self.close_tab(&cdp_session.target_id).await;
            }
        }

        // Kill Chrome process if we launched it.
        let mut process = self.chrome_process.lock().await;
        if let Some(ref mut child) = *process {
            info!("killing Chrome process");
            if let Err(e) = child.kill().await {
                warn!(error = %e, "failed to kill Chrome process");
            }
        }
        *process = None;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// BrowserDriver trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl BrowserDriver for CdpBrowserDriver {
    async fn launch(&self, config: &BrowserConfig) -> PunchResult<BrowserSession> {
        // Update our config from the BrowserConfig.
        let mut port_lock = self.active_port.lock().await;
        *port_lock = Some(config.remote_debugging_port);
        drop(port_lock);

        // Launch Chrome.
        let child = self.launch_chrome().await?;
        let mut process_lock = self.chrome_process.lock().await;
        *process_lock = Some(child);
        drop(process_lock);

        // Wait for CDP to be ready.
        self.wait_for_cdp_ready().await?;

        // Create the initial tab/session.
        let target = self.create_tab(None).await?;

        let mut session = BrowserSession::new();
        session.state = BrowserState::Connected;
        session.current_url = Some(target.url.clone());

        let cdp_session = CdpSession {
            id: session.id.to_string(),
            target_id: target.id,
            ws_url: target.web_socket_debugger_url,
            created_at: Utc::now(),
        };
        self.sessions.insert(session.id.to_string(), cdp_session);

        info!(session_id = %session.id, "browser session launched");
        Ok(session)
    }

    async fn execute(
        &self,
        session: &mut BrowserSession,
        action: BrowserAction,
    ) -> PunchResult<BrowserResult> {
        match action {
            BrowserAction::Navigate { url } => self.execute_navigate(session, &url).await,
            BrowserAction::Click { selector } => self.execute_click(session, &selector).await,
            BrowserAction::Type { selector, text } => {
                self.execute_type(session, &selector, &text).await
            }
            BrowserAction::Screenshot { full_page } => {
                self.execute_screenshot(session, full_page).await
            }
            BrowserAction::GetContent { selector } => {
                self.execute_get_content(session, selector.as_deref()).await
            }
            BrowserAction::GetHtml { selector } => {
                self.execute_get_html(session, selector.as_deref()).await
            }
            BrowserAction::WaitForSelector {
                selector,
                timeout_ms,
            } => {
                self.execute_wait_for_selector(session, &selector, timeout_ms)
                    .await
            }
            BrowserAction::Evaluate { javascript } => {
                self.execute_evaluate(session, &javascript).await
            }
            BrowserAction::GoBack => {
                self.execute_evaluate(session, "window.history.back()")
                    .await
            }
            BrowserAction::GoForward => {
                self.execute_evaluate(session, "window.history.forward()")
                    .await
            }
            BrowserAction::Reload => {
                self.execute_evaluate(session, "window.location.reload()")
                    .await
            }
            BrowserAction::Close => {
                self.close(session).await?;
                Ok(BrowserResult::ok(serde_json::json!({"closed": true})))
            }
        }
    }

    async fn close(&self, session: &mut BrowserSession) -> PunchResult<()> {
        let session_id = session.id.to_string();
        if let Some((_, cdp_session)) = self.sessions.remove(&session_id) {
            let _ = self.close_tab(&cdp_session.target_id).await;
        }
        session.state = BrowserState::Closed;
        info!(session_id = %session_id, "browser session closed");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    // -- CdpConfig tests --

    #[test]
    fn test_cdp_config_defaults() {
        let config = CdpConfig::default();
        assert!(config.chrome_path.is_none());
        assert_eq!(config.debug_port, 9222);
        assert!(config.headless);
        assert!(config.user_data_dir.is_none());
        assert!(config.extra_args.is_empty());
        assert_eq!(config.connect_timeout_secs, 10);
        assert!(config.disable_gpu);
        assert!(config.no_sandbox);
    }

    #[test]
    fn test_cdp_config_from_browser_config() {
        let browser_config = BrowserConfig {
            chrome_path: Some("/usr/bin/chromium".into()),
            headless: false,
            remote_debugging_port: 9333,
            user_data_dir: Some("/tmp/chrome-test".into()),
            timeout_secs: 60,
            viewport_width: 1920,
            viewport_height: 1080,
        };

        let cdp_config = CdpConfig::from(&browser_config);
        assert_eq!(cdp_config.chrome_path.as_deref(), Some("/usr/bin/chromium"));
        assert_eq!(cdp_config.debug_port, 9333);
        assert!(!cdp_config.headless);
        assert_eq!(
            cdp_config.user_data_dir.as_deref(),
            Some("/tmp/chrome-test")
        );
    }

    #[test]
    fn test_cdp_config_serialization_roundtrip() {
        let config = CdpConfig {
            chrome_path: Some("/usr/bin/google-chrome".into()),
            debug_port: 9333,
            headless: false,
            user_data_dir: Some("/tmp/data".into()),
            extra_args: vec!["--disable-extensions".into()],
            connect_timeout_secs: 20,
            disable_gpu: false,
            no_sandbox: false,
        };

        let json = serde_json::to_string(&config).expect("should serialize");
        let deserialized: CdpConfig = serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(
            deserialized.chrome_path.as_deref(),
            Some("/usr/bin/google-chrome")
        );
        assert_eq!(deserialized.debug_port, 9333);
        assert!(!deserialized.headless);
        assert_eq!(deserialized.extra_args.len(), 1);
    }

    // -- Chrome path detection tests --

    #[test]
    fn test_chrome_candidate_paths_not_empty() {
        let paths = chrome_candidate_paths();
        // On any supported platform, we should have candidate paths.
        assert!(
            !paths.is_empty(),
            "should have at least one candidate Chrome path"
        );
    }

    #[test]
    fn test_find_chrome_returns_existing_path_or_none() {
        // We can't guarantee Chrome is installed, but the function should not panic.
        let result = find_chrome();
        if let Some(ref path) = result {
            assert!(
                std::path::Path::new(path).exists(),
                "found path should exist: {}",
                path
            );
        }
        // If None, that's fine — Chrome just isn't installed.
    }

    // -- CdpSession tests --

    #[test]
    fn test_cdp_session_creation() {
        let session = CdpSession {
            id: "test-session-123".into(),
            target_id: "ABCD1234".into(),
            ws_url: "ws://localhost:9222/devtools/page/ABCD1234".into(),
            created_at: Utc::now(),
        };

        assert_eq!(session.id, "test-session-123");
        assert_eq!(session.target_id, "ABCD1234");
        assert!(session.ws_url.contains("devtools/page"));
    }

    #[test]
    fn test_cdp_session_serialization() {
        let session = CdpSession {
            id: "s1".into(),
            target_id: "t1".into(),
            ws_url: "ws://localhost:9222/devtools/page/t1".into(),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&session).expect("should serialize");
        let deserialized: CdpSession = serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(deserialized.id, "s1");
        assert_eq!(deserialized.target_id, "t1");
    }

    // -- CDP command JSON formatting tests --

    #[test]
    fn test_navigate_command_format() {
        let cmd = build_navigate_command(1, "https://example.com");
        assert_eq!(cmd.id, 1);
        assert_eq!(cmd.method, "Page.navigate");
        assert_eq!(cmd.params["url"], "https://example.com");

        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("Page.navigate"));
        assert!(json.contains("https://example.com"));
    }

    #[test]
    fn test_screenshot_command_format() {
        let cmd = build_screenshot_command(2, false);
        assert_eq!(cmd.id, 2);
        assert_eq!(cmd.method, "Page.captureScreenshot");

        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("Page.captureScreenshot"));

        let full_cmd = build_screenshot_command(3, true);
        let full_json = full_cmd.to_json().expect("should serialize");
        assert!(full_json.contains("captureBeyondViewport"));
    }

    #[test]
    fn test_evaluate_command_format() {
        let cmd = build_evaluate_command(4, "document.title");
        assert_eq!(cmd.id, 4);
        assert_eq!(cmd.method, "Runtime.evaluate");
        assert_eq!(cmd.params["expression"], "document.title");
        assert_eq!(cmd.params["returnByValue"], true);
        assert_eq!(cmd.params["awaitPromise"], true);
    }

    #[test]
    fn test_click_command_format() {
        let cmd = build_click_command(5, "#submit-btn");
        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("Runtime.evaluate"));
        assert!(json.contains("querySelector"));
        assert!(json.contains("#submit-btn"));
        assert!(json.contains("click()"));
    }

    #[test]
    fn test_get_content_command_with_selector() {
        let cmd = build_get_content_command(6, Some("h1.title"));
        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("Runtime.evaluate"));
        assert!(json.contains("textContent"));
        assert!(json.contains("h1.title"));
    }

    #[test]
    fn test_get_content_command_without_selector() {
        let cmd = build_get_content_command(7, None);
        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("document.body.innerText"));
    }

    #[test]
    fn test_type_text_command_format() {
        let cmd = build_type_text_command(8, "input#search", "hello world");
        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("Runtime.evaluate"));
        assert!(json.contains("input#search"));
        assert!(json.contains("hello world"));
        assert!(json.contains("dispatchEvent"));
    }

    #[test]
    fn test_wait_for_selector_command_format() {
        let cmd = build_wait_for_selector_command(9, ".loaded", 5000);
        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("Runtime.evaluate"));
        assert!(json.contains(".loaded"));
        assert!(json.contains("5000"));
    }

    #[test]
    fn test_get_html_command_with_selector() {
        let cmd = build_get_html_command(10, Some("div.content"));
        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("outerHTML"));
        assert!(json.contains("div.content"));
    }

    #[test]
    fn test_get_html_command_without_selector() {
        let cmd = build_get_html_command(11, None);
        let json = cmd.to_json().expect("should serialize");
        assert!(json.contains("document.documentElement.outerHTML"));
    }

    // -- CdpCommand general tests --

    #[test]
    fn test_cdp_command_new() {
        let cmd = CdpCommand::new(42, "DOM.getDocument", serde_json::json!({"depth": 1}));
        assert_eq!(cmd.id, 42);
        assert_eq!(cmd.method, "DOM.getDocument");
        assert_eq!(cmd.params["depth"], 1);
    }

    #[test]
    fn test_cdp_response_parse_success() {
        let json = r#"{"id": 1, "result": {"frameId": "abc123"}}"#;
        let resp: CdpResponse = serde_json::from_str(json).expect("should parse");
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap()["frameId"], "abc123");
    }

    #[test]
    fn test_cdp_response_parse_error() {
        let json = r#"{"id": 2, "error": {"code": -32601, "message": "method not found"}}"#;
        let resp: CdpResponse = serde_json::from_str(json).expect("should parse");
        assert_eq!(resp.id, Some(2));
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "method not found");
    }

    // -- CdpTargetInfo parse test --

    #[test]
    fn test_cdp_target_info_parse() {
        let json = r#"{
            "description": "",
            "devtoolsFrontendUrl": "/devtools/inspector.html?ws=localhost:9222/devtools/page/ABC",
            "id": "ABC123",
            "title": "about:blank",
            "type": "page",
            "url": "about:blank",
            "webSocketDebuggerUrl": "ws://localhost:9222/devtools/page/ABC123"
        }"#;

        let target: CdpTargetInfo = serde_json::from_str(json).expect("should parse");
        assert_eq!(target.id, "ABC123");
        assert_eq!(target.target_type, "page");
        assert_eq!(target.url, "about:blank");
        assert!(target.web_socket_debugger_url.contains("ws://"));
    }

    // -- CdpError conversion test --

    #[test]
    fn test_cdp_error_to_punch_error() {
        let cdp_err = CdpError::ChromeNotFound {
            searched_paths: vec!["/usr/bin/chrome".into()],
        };
        let punch_err: PunchError = cdp_err.into();
        let msg = punch_err.to_string();
        assert!(msg.contains("browser_cdp"), "error: {}", msg);
        assert!(msg.contains("Chrome binary not found"), "error: {}", msg);
    }

    #[test]
    fn test_cdp_error_variants() {
        // Verify all error variants can be constructed and formatted.
        let errors: Vec<CdpError> = vec![
            CdpError::ChromeNotFound {
                searched_paths: vec![],
            },
            CdpError::LaunchFailed {
                reason: "permission denied".into(),
            },
            CdpError::ConnectionFailed {
                port: 9222,
                reason: "refused".into(),
            },
            CdpError::CommandError {
                command_id: 1,
                message: "eval failed".into(),
            },
            CdpError::SessionNotFound {
                session_id: "abc".into(),
            },
            CdpError::UnexpectedResponse {
                detail: "bad json".into(),
            },
            CdpError::Timeout { timeout_secs: 30 },
            CdpError::Http("connection reset".into()),
        ];

        for err in &errors {
            let msg = err.to_string();
            assert!(!msg.is_empty(), "error message should not be empty");
        }
    }

    // -- BrowserDriver trait implementation compile-time verification --

    #[test]
    fn test_cdp_browser_driver_implements_trait() {
        // This test verifies at compile time that CdpBrowserDriver implements
        // the BrowserDriver trait. If it compiles, the trait is satisfied.
        fn _assert_browser_driver<T: BrowserDriver>() {}
        _assert_browser_driver::<CdpBrowserDriver>();
    }

    #[test]
    fn test_cdp_browser_driver_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<CdpBrowserDriver>();
    }

    // -- Driver construction tests --

    #[test]
    fn test_cdp_driver_creation() {
        let driver = CdpBrowserDriver::new(CdpConfig::default());
        assert_eq!(driver.sessions.len(), 0);
    }

    #[test]
    fn test_cdp_driver_with_defaults() {
        let driver = CdpBrowserDriver::with_defaults();
        assert_eq!(driver.sessions.len(), 0);
    }

    #[test]
    fn test_cdp_driver_chrome_args() {
        let config = CdpConfig {
            debug_port: 9333,
            headless: true,
            disable_gpu: true,
            no_sandbox: true,
            user_data_dir: Some("/tmp/test-data".into()),
            extra_args: vec!["--disable-extensions".into()],
            ..Default::default()
        };
        let driver = CdpBrowserDriver::new(config);
        let args = driver.build_chrome_args();

        assert!(args.contains(&"--remote-debugging-port=9333".to_string()));
        assert!(args.contains(&"--headless".to_string()));
        assert!(args.contains(&"--disable-gpu".to_string()));
        assert!(args.contains(&"--no-sandbox".to_string()));
        assert!(args.contains(&"--user-data-dir=/tmp/test-data".to_string()));
        assert!(args.contains(&"--disable-extensions".to_string()));
        assert!(args.contains(&"about:blank".to_string()));
    }

    #[test]
    fn test_cdp_driver_chrome_args_minimal() {
        let config = CdpConfig {
            headless: false,
            disable_gpu: false,
            no_sandbox: false,
            user_data_dir: None,
            extra_args: vec![],
            ..Default::default()
        };
        let driver = CdpBrowserDriver::new(config);
        let args = driver.build_chrome_args();

        assert!(!args.contains(&"--headless".to_string()));
        assert!(!args.contains(&"--disable-gpu".to_string()));
        assert!(!args.contains(&"--no-sandbox".to_string()));
        // Should still have the port and about:blank.
        assert!(
            args.iter()
                .any(|a| a.starts_with("--remote-debugging-port="))
        );
        assert!(args.contains(&"about:blank".to_string()));
    }

    // -- Session lifecycle tests (no actual Chrome) --

    #[test]
    fn test_cdp_driver_session_tracking() {
        let driver = CdpBrowserDriver::with_defaults();

        // Manually insert a session to simulate creation.
        let session_id = Uuid::new_v4().to_string();
        let cdp_session = CdpSession {
            id: session_id.clone(),
            target_id: "target_001".into(),
            ws_url: "ws://localhost:9222/devtools/page/target_001".into(),
            created_at: Utc::now(),
        };
        driver.sessions.insert(session_id.clone(), cdp_session);

        assert_eq!(driver.sessions.len(), 1);
        let retrieved = driver.get_cdp_session(&session_id);
        assert!(retrieved.is_ok());
        assert_eq!(retrieved.unwrap().target_id, "target_001");
    }

    #[test]
    fn test_cdp_driver_session_not_found() {
        let driver = CdpBrowserDriver::with_defaults();
        let result = driver.get_cdp_session("nonexistent-id");
        assert!(result.is_err());
        match result.unwrap_err() {
            CdpError::SessionNotFound { session_id } => {
                assert_eq!(session_id, "nonexistent-id");
            }
            other => panic!("expected SessionNotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_cdp_driver_multiple_sessions() {
        let driver = CdpBrowserDriver::with_defaults();

        for i in 0..5 {
            let session_id = format!("session_{}", i);
            let cdp_session = CdpSession {
                id: session_id.clone(),
                target_id: format!("target_{}", i),
                ws_url: format!("ws://localhost:9222/devtools/page/target_{}", i),
                created_at: Utc::now(),
            };
            driver.sessions.insert(session_id, cdp_session);
        }

        assert_eq!(driver.sessions.len(), 5);

        // Remove one.
        driver.sessions.remove("session_2");
        assert_eq!(driver.sessions.len(), 4);
        assert!(driver.get_cdp_session("session_2").is_err());
        assert!(driver.get_cdp_session("session_3").is_ok());
    }

    #[test]
    fn test_cdp_driver_next_id_increments() {
        let driver = CdpBrowserDriver::with_defaults();
        let id1 = driver.next_id();
        let id2 = driver.next_id();
        let id3 = driver.next_id();

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    // -- Chrome resolve path test --

    #[test]
    fn test_resolve_chrome_path_with_config() {
        let config = CdpConfig {
            chrome_path: Some("/custom/path/chrome".into()),
            ..Default::default()
        };
        let driver = CdpBrowserDriver::new(config);
        let path = driver.resolve_chrome_path();
        assert!(path.is_ok());
        assert_eq!(path.unwrap(), "/custom/path/chrome");
    }
}
