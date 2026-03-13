//! Desktop application lifecycle management.
//!
//! `DesktopApp` manages the Arena server process, browser opening, and
//! provides a control API for the desktop binary.

use std::process::Command as StdCommand;
use tracing::{error, info, warn};

use crate::commands::ArenaClient;
use crate::state::{DesktopState, Theme};

/// Configuration for the desktop application parsed from CLI args.
#[derive(Debug, Clone)]
pub struct DesktopConfig {
    /// Port for the Arena API server.
    pub port: u16,
    /// Whether to auto-open the browser.
    pub open_browser: bool,
    /// Theme preference.
    pub theme: Theme,
    /// API key for Arena authentication (optional).
    pub api_key: Option<String>,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            port: 6660,
            open_browser: true,
            theme: Theme::Dark,
            api_key: None,
        }
    }
}

/// The desktop application.
pub struct DesktopApp {
    /// Application state.
    state: DesktopState,
    /// Arena HTTP client.
    client: ArenaClient,
    /// Application configuration.
    config: DesktopConfig,
}

impl DesktopApp {
    /// Create a new desktop application with the given configuration.
    pub fn new(config: DesktopConfig) -> Self {
        let arena_url = format!("http://localhost:{}", config.port);
        let state = DesktopState {
            auto_open_browser: config.open_browser,
            theme: config.theme,
            ..DesktopState::new(arena_url.clone())
        };
        let client = ArenaClient::new(&arena_url);

        Self {
            state,
            client,
            config,
        }
    }

    /// Get a reference to the current state.
    pub fn state(&self) -> &DesktopState {
        &self.state
    }

    /// Get a mutable reference to the current state.
    pub fn state_mut(&mut self) -> &mut DesktopState {
        &mut self.state
    }

    /// Get a reference to the Arena client.
    pub fn client(&self) -> &ArenaClient {
        &self.client
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &DesktopConfig {
        &self.config
    }

    /// Wait for the Arena to become healthy, polling at intervals.
    ///
    /// Returns `true` if the Arena became reachable within the timeout,
    /// `false` otherwise.
    pub async fn wait_for_arena(&mut self, timeout_secs: u64) -> bool {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        info!(
            url = %self.state.arena_url,
            timeout_secs = timeout_secs,
            "waiting for arena to start"
        );

        while start.elapsed() < timeout {
            match self.client.health_check().await {
                Ok(true) => {
                    self.state.mark_connected();
                    info!("arena is healthy");
                    return true;
                }
                _ => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }

        warn!("arena did not become healthy within timeout");
        false
    }

    /// Open the system default browser to the Arena dashboard.
    pub fn open_browser(&self) -> bool {
        let url = self.state.dashboard_url();
        info!(url = %url, "opening browser");

        let result = open_url_in_browser(&url);
        if !result {
            error!(url = %url, "failed to open browser");
        }
        result
    }

    /// Print startup banner to the terminal.
    pub fn print_banner(&self) {
        println!();
        println!("  ╔═══════════════════════════════════════════╗");
        println!("  ║          PUNCH DESKTOP                    ║");
        println!("  ║     The Agent Combat System               ║");
        println!("  ╚═══════════════════════════════════════════╝");
        println!();
        println!("  Arena URL:  {}", self.state.arena_url);
        println!("  Dashboard:  {}", self.state.dashboard_url());
        println!("  Theme:      {}", self.state.theme);
        println!();
    }
}

/// Open a URL in the system default browser.
fn open_url_in_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        StdCommand::new("open").arg(url).spawn().is_ok()
    }

    #[cfg(target_os = "linux")]
    {
        StdCommand::new("xdg-open").arg(url).spawn().is_ok()
    }

    #[cfg(target_os = "windows")]
    {
        StdCommand::new("cmd")
            .args(["/C", "start", url])
            .spawn()
            .is_ok()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        warn!(url = %url, "no browser opener for this platform");
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desktop_config_default() {
        let config = DesktopConfig::default();
        assert_eq!(config.port, 6660);
        assert!(config.open_browser);
        assert_eq!(config.theme, Theme::Dark);
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_desktop_app_new() {
        let config = DesktopConfig {
            port: 7777,
            open_browser: false,
            theme: Theme::Light,
            api_key: Some("test-key".to_string()),
        };
        let app = DesktopApp::new(config);
        assert_eq!(app.state().arena_url, "http://localhost:7777");
        assert!(!app.state().auto_open_browser);
        assert_eq!(app.state().theme, Theme::Light);
        assert_eq!(app.client().base_url(), "http://localhost:7777");
    }

    #[test]
    fn test_desktop_app_state_access() {
        let app = DesktopApp::new(DesktopConfig::default());
        assert!(!app.state().arena_running);
        assert!(!app.state().connected);
    }

    #[test]
    fn test_desktop_app_state_mut() {
        let mut app = DesktopApp::new(DesktopConfig::default());
        app.state_mut().mark_connected();
        assert!(app.state().arena_running);
        assert!(app.state().connected);
    }

    #[tokio::test]
    async fn test_wait_for_arena_timeout() {
        let mut app = DesktopApp::new(DesktopConfig {
            port: 19998,
            ..DesktopConfig::default()
        });
        // Should timeout quickly since nothing is listening.
        let result = app.wait_for_arena(1).await;
        assert!(!result);
        assert!(!app.state().connected);
    }

    #[test]
    fn test_config_accessor() {
        let config = DesktopConfig {
            port: 8080,
            ..DesktopConfig::default()
        };
        let app = DesktopApp::new(config);
        assert_eq!(app.config().port, 8080);
    }

    #[test]
    fn test_desktop_app_dashboard_url() {
        let app = DesktopApp::new(DesktopConfig::default());
        assert_eq!(
            app.state().dashboard_url(),
            "http://localhost:6660/dashboard"
        );
    }

    #[test]
    fn test_desktop_app_theme_setting() {
        let config = DesktopConfig {
            theme: Theme::System,
            ..DesktopConfig::default()
        };
        let app = DesktopApp::new(config);
        assert_eq!(app.state().theme, Theme::System);
    }

    #[test]
    fn test_desktop_app_custom_port() {
        let config = DesktopConfig {
            port: 3000,
            ..DesktopConfig::default()
        };
        let app = DesktopApp::new(config);
        assert_eq!(app.state().arena_url, "http://localhost:3000");
        assert_eq!(app.client().base_url(), "http://localhost:3000");
    }

    #[test]
    fn test_desktop_app_no_auto_open() {
        let config = DesktopConfig {
            open_browser: false,
            ..DesktopConfig::default()
        };
        let app = DesktopApp::new(config);
        assert!(!app.state().auto_open_browser);
    }

    #[test]
    fn test_desktop_config_api_key() {
        let config = DesktopConfig {
            api_key: Some("my-secret".to_string()),
            ..DesktopConfig::default()
        };
        assert_eq!(config.api_key, Some("my-secret".to_string()));
    }
}
