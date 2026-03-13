//! Browser automation types — ring-side view into the web.
//!
//! This module defines the types, traits, and pool management for CDP-based
//! browser automation. Agents use these scouting moves to navigate web pages,
//! take screenshots, click elements, and extract content. The actual CDP
//! WebSocket driver is plugged in separately — this module provides the
//! contract and the session arena.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for launching a browser instance.
///
/// Sensible defaults let a fighter step into the ring without fuss —
/// headless Chrome on port 9222, 30-second timeout, standard viewport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Path to the Chrome/Chromium binary. `None` means auto-detect.
    pub chrome_path: Option<String>,
    /// Run headless (no visible window). Default: `true`.
    pub headless: bool,
    /// Remote debugging port for CDP. Default: `9222`.
    pub remote_debugging_port: u16,
    /// Custom user-data directory. `None` uses a temp directory.
    pub user_data_dir: Option<String>,
    /// Per-action timeout in seconds. Default: `30`.
    pub timeout_secs: u64,
    /// Viewport width in pixels. Default: `1280`.
    pub viewport_width: u32,
    /// Viewport height in pixels. Default: `720`.
    pub viewport_height: u32,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            chrome_path: None,
            headless: true,
            remote_debugging_port: 9222,
            user_data_dir: None,
            timeout_secs: 30,
            viewport_width: 1280,
            viewport_height: 720,
        }
    }
}

// ---------------------------------------------------------------------------
// Session & State
// ---------------------------------------------------------------------------

/// The current state of a browser session in the ring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "detail")]
pub enum BrowserState {
    /// Browser process is starting up — warming up before the bout.
    Starting,
    /// Connected to CDP endpoint — ready to receive orders.
    Connected,
    /// A navigation is in progress — fighter is on the move.
    Navigating,
    /// Page loaded and ready for interaction — stance is set.
    Ready,
    /// An error occurred — fighter took a hit.
    Error(String),
    /// Session has been closed — bout is over.
    Closed,
}

/// A live browser session — one fighter's ring-side view into the web.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSession {
    /// Unique session identifier.
    pub id: Uuid,
    /// When this session was created.
    pub created_at: DateTime<Utc>,
    /// The URL currently loaded, if any.
    pub current_url: Option<String>,
    /// The page title, if available.
    pub page_title: Option<String>,
    /// Current session state.
    pub state: BrowserState,
}

impl BrowserSession {
    /// Create a new session in the `Starting` state.
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            current_url: None,
            page_title: None,
            state: BrowserState::Starting,
        }
    }
}

impl Default for BrowserSession {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Actions & Results
// ---------------------------------------------------------------------------

/// An action the agent wants to perform in the browser — a scouting move.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum BrowserAction {
    /// Navigate to a URL.
    Navigate { url: String },
    /// Click an element matching the CSS selector.
    Click { selector: String },
    /// Type text into an element matching the CSS selector.
    Type { selector: String, text: String },
    /// Take a screenshot (full page or viewport only).
    Screenshot { full_page: bool },
    /// Get the text content of an element (or the whole page).
    GetContent { selector: Option<String> },
    /// Get the HTML of an element (or the whole page).
    GetHtml { selector: Option<String> },
    /// Wait for an element matching the selector to appear.
    WaitForSelector { selector: String, timeout_ms: u64 },
    /// Execute arbitrary JavaScript in the page context.
    Evaluate { javascript: String },
    /// Navigate back in history.
    GoBack,
    /// Navigate forward in history.
    GoForward,
    /// Reload the current page.
    Reload,
    /// Close the browser session.
    Close,
}

/// The result of executing a browser action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserResult {
    /// Whether the action succeeded.
    pub success: bool,
    /// Result data — screenshot base64, extracted text, JS return value, etc.
    pub data: serde_json::Value,
    /// The page URL after the action completed.
    pub page_url: Option<String>,
    /// The page title after the action completed.
    pub page_title: Option<String>,
    /// How long the action took in milliseconds.
    pub duration_ms: u64,
    /// Error message if the action failed.
    pub error: Option<String>,
}

impl BrowserResult {
    /// Construct a successful result with the given data.
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            success: true,
            data,
            page_url: None,
            page_title: None,
            duration_ms: 0,
            error: None,
        }
    }

    /// Construct a failed result with an error message.
    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: serde_json::Value::Null,
            page_url: None,
            page_title: None,
            duration_ms: 0,
            error: Some(message.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Driver trait
// ---------------------------------------------------------------------------

/// Trait for a browser automation driver — the engine behind the punches.
///
/// Implementations handle the actual CDP WebSocket communication. This trait
/// defines the contract so drivers can be swapped or mocked in tests.
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    /// Launch a browser instance and return a connected session.
    async fn launch(&self, config: &BrowserConfig) -> PunchResult<BrowserSession>;

    /// Execute a browser action within the given session.
    async fn execute(
        &self,
        session: &mut BrowserSession,
        action: BrowserAction,
    ) -> PunchResult<BrowserResult>;

    /// Close the browser session and clean up resources.
    async fn close(&self, session: &mut BrowserSession) -> PunchResult<()>;
}

// ---------------------------------------------------------------------------
// Session Pool
// ---------------------------------------------------------------------------

/// A pool of browser sessions — the roster of active ring-side scouts.
///
/// Manages concurrent browser sessions with an upper bound, backed by
/// `DashMap` for lock-free concurrent access.
pub struct BrowserPool {
    /// Active sessions keyed by their UUID.
    sessions: DashMap<Uuid, BrowserSession>,
    /// Configuration applied to new sessions.
    config: BrowserConfig,
    /// Maximum number of concurrent sessions.
    max_sessions: usize,
}

impl BrowserPool {
    /// Create a new browser pool with the given config and session limit.
    pub fn new(config: BrowserConfig, max_sessions: usize) -> Self {
        Self {
            sessions: DashMap::new(),
            config,
            max_sessions,
        }
    }

    /// Retrieve a clone of a session by its ID.
    pub fn get_session(&self, id: &Uuid) -> Option<BrowserSession> {
        self.sessions.get(id).map(|entry| entry.value().clone())
    }

    /// List all active sessions.
    pub fn active_sessions(&self) -> Vec<BrowserSession> {
        self.sessions
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Return the number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Create a new session and add it to the pool.
    ///
    /// Returns an error if the pool is at capacity — no room in the ring.
    pub fn create_session(&self) -> PunchResult<BrowserSession> {
        if self.sessions.len() >= self.max_sessions {
            return Err(PunchError::Tool {
                tool: "browser".into(),
                message: format!(
                    "browser pool at capacity ({}/{})",
                    self.sessions.len(),
                    self.max_sessions
                ),
            });
        }

        let session = BrowserSession::new();
        self.sessions.insert(session.id, session.clone());
        Ok(session)
    }

    /// Close and remove a session from the pool.
    pub fn close_session(&self, id: &Uuid) -> PunchResult<()> {
        self.sessions.remove(id).ok_or_else(|| PunchError::Tool {
            tool: "browser".into(),
            message: format!("session {} not found in pool", id),
        })?;
        Ok(())
    }

    /// Close all sessions — clear the ring.
    pub fn close_all(&self) {
        self.sessions.clear();
    }

    /// Get a reference to the pool's browser configuration.
    pub fn config(&self) -> &BrowserConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_config_defaults() {
        let config = BrowserConfig::default();
        assert!(config.headless);
        assert_eq!(config.remote_debugging_port, 9222);
        assert!(config.chrome_path.is_none());
        assert!(config.user_data_dir.is_none());
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.viewport_width, 1280);
        assert_eq!(config.viewport_height, 720);
    }

    #[test]
    fn test_browser_session_creation() {
        let session = BrowserSession::new();
        assert_eq!(session.state, BrowserState::Starting);
        assert!(session.current_url.is_none());
        assert!(session.page_title.is_none());
    }

    #[test]
    fn test_browser_pool_create_session() {
        let pool = BrowserPool::new(BrowserConfig::default(), 5);
        assert_eq!(pool.session_count(), 0);

        let session = pool.create_session().expect("should create session");
        assert_eq!(pool.session_count(), 1);

        let retrieved = pool.get_session(&session.id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.expect("should exist").id, session.id);
    }

    #[test]
    fn test_browser_pool_max_sessions_enforced() {
        let pool = BrowserPool::new(BrowserConfig::default(), 2);

        pool.create_session().expect("session 1");
        pool.create_session().expect("session 2");

        let result = pool.create_session();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("at capacity"), "error: {}", err);
    }

    #[test]
    fn test_browser_pool_close_session() {
        let pool = BrowserPool::new(BrowserConfig::default(), 5);
        let session = pool.create_session().expect("should create session");
        assert_eq!(pool.session_count(), 1);

        pool.close_session(&session.id)
            .expect("should close session");
        assert_eq!(pool.session_count(), 0);

        // Closing again should fail — fighter already left the ring.
        let result = pool.close_session(&session.id);
        assert!(result.is_err());
    }

    #[test]
    fn test_browser_pool_close_all() {
        let pool = BrowserPool::new(BrowserConfig::default(), 10);
        for _ in 0..5 {
            pool.create_session().expect("should create session");
        }
        assert_eq!(pool.session_count(), 5);

        pool.close_all();
        assert_eq!(pool.session_count(), 0);
    }

    #[test]
    fn test_browser_action_serialization() {
        let action = BrowserAction::Navigate {
            url: "https://example.com".into(),
        };
        let json = serde_json::to_string(&action).expect("should serialize");
        assert!(json.contains("navigate"));
        assert!(json.contains("https://example.com"));

        let deserialized: BrowserAction = serde_json::from_str(&json).expect("should deserialize");
        match deserialized {
            BrowserAction::Navigate { url } => assert_eq!(url, "https://example.com"),
            _ => panic!("expected Navigate variant"),
        }
    }

    #[test]
    fn test_browser_result_construction() {
        let ok_result = BrowserResult::ok(serde_json::json!({"html": "<h1>Hello</h1>"}));
        assert!(ok_result.success);
        assert!(ok_result.error.is_none());
        assert_eq!(ok_result.data["html"], "<h1>Hello</h1>");

        let fail_result = BrowserResult::fail("element not found");
        assert!(!fail_result.success);
        assert_eq!(fail_result.error.as_deref(), Some("element not found"));
        assert_eq!(fail_result.data, serde_json::Value::Null);
    }

    #[test]
    fn test_browser_state_transitions() {
        // Verify state variants can be constructed and compared.
        let states = vec![
            BrowserState::Starting,
            BrowserState::Connected,
            BrowserState::Navigating,
            BrowserState::Ready,
            BrowserState::Error("timeout".into()),
            BrowserState::Closed,
        ];

        // Each state is distinct.
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }

        // Error states with different messages are distinct.
        let err1 = BrowserState::Error("timeout".into());
        let err2 = BrowserState::Error("crash".into());
        assert_ne!(err1, err2);
    }

    #[test]
    fn test_browser_config_serialization_roundtrip() {
        let config = BrowserConfig {
            chrome_path: Some("/usr/bin/chromium".into()),
            headless: false,
            remote_debugging_port: 9333,
            user_data_dir: Some("/tmp/chrome-data".into()),
            timeout_secs: 60,
            viewport_width: 1920,
            viewport_height: 1080,
        };

        let json = serde_json::to_string(&config).expect("should serialize config");
        let deserialized: BrowserConfig =
            serde_json::from_str(&json).expect("should deserialize config");

        assert_eq!(
            deserialized.chrome_path.as_deref(),
            Some("/usr/bin/chromium")
        );
        assert!(!deserialized.headless);
        assert_eq!(deserialized.remote_debugging_port, 9333);
        assert_eq!(
            deserialized.user_data_dir.as_deref(),
            Some("/tmp/chrome-data")
        );
        assert_eq!(deserialized.timeout_secs, 60);
        assert_eq!(deserialized.viewport_width, 1920);
        assert_eq!(deserialized.viewport_height, 1080);
    }

    #[test]
    fn test_browser_pool_active_sessions() {
        let pool = BrowserPool::new(BrowserConfig::default(), 5);
        let s1 = pool.create_session().expect("session 1");
        let s2 = pool.create_session().expect("session 2");

        let active = pool.active_sessions();
        assert_eq!(active.len(), 2);

        let ids: Vec<Uuid> = active.iter().map(|s| s.id).collect();
        assert!(ids.contains(&s1.id));
        assert!(ids.contains(&s2.id));
    }

    #[test]
    fn test_browser_session_default() {
        let session = BrowserSession::default();
        assert_eq!(session.state, BrowserState::Starting);
    }
}
