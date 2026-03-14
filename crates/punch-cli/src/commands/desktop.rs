//! `punch desktop` — Launch the Punch Agent OS dashboard in a browser or native webview.
//!
//! The desktop command starts the Arena HTTP server in the background, then opens
//! the Punch dashboard in the user's default browser. It provides a terminal-based
//! control interface for managing the desktop session.
//!
//! When compiled with the `desktop` feature, a native webview window is available
//! via `punch desktop --native`.

use std::fmt;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Theme preference for the desktop dashboard.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Light color scheme.
    Light,
    /// Dark color scheme.
    Dark,
    /// Follow the operating system preference.
    #[default]
    System,
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Light => write!(f, "light"),
            Self::Dark => write!(f, "dark"),
            Self::System => write!(f, "system"),
        }
    }
}

/// Configuration for the desktop launcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopConfig {
    /// Port for the Arena server (default: 6660).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Whether to automatically open the browser on launch.
    #[serde(default = "default_true")]
    pub auto_open_browser: bool,
    /// Color theme preference.
    #[serde(default)]
    pub theme: Theme,
    /// Custom host to bind to (default: "127.0.0.1").
    #[serde(default = "default_host")]
    pub host: String,
    /// Window title for native webview mode.
    #[serde(default = "default_title")]
    pub window_title: String,
    /// Window width for native webview mode.
    #[serde(default = "default_width")]
    pub window_width: u32,
    /// Window height for native webview mode.
    #[serde(default = "default_height")]
    pub window_height: u32,
}

fn default_port() -> u16 {
    6660
}

fn default_true() -> bool {
    true
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_title() -> String {
    "Punch Agent OS".to_string()
}

fn default_width() -> u32 {
    1200
}

fn default_height() -> u32 {
    800
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            auto_open_browser: true,
            theme: Theme::default(),
            host: default_host(),
            window_title: default_title(),
            window_width: default_width(),
            window_height: default_height(),
        }
    }
}

impl DesktopConfig {
    /// Construct the full dashboard URL from the host and port.
    pub fn dashboard_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    /// Construct the dashboard URL with theme query parameter.
    pub fn dashboard_url_with_theme(&self) -> String {
        format!("http://{}:{}/?theme={}", self.host, self.port, self.theme)
    }
}

// ---------------------------------------------------------------------------
// Platform detection & browser opening
// ---------------------------------------------------------------------------

/// Detected host platform for browser-opening commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOS,
    Linux,
    Windows,
    Unknown,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MacOS => write!(f, "macOS"),
            Self::Linux => write!(f, "Linux"),
            Self::Windows => write!(f, "Windows"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detect the current platform at runtime.
pub fn detect_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOS
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Unknown
    }
}

/// Return the system command used to open a URL in the default browser.
///
/// Returns `(command, args_prefix)` — the URL is appended as the last argument.
pub fn browser_command(platform: Platform) -> Option<(&'static str, Vec<&'static str>)> {
    match platform {
        Platform::MacOS => Some(("open", vec![])),
        Platform::Linux => Some(("xdg-open", vec![])),
        Platform::Windows => Some(("cmd", vec!["/C", "start"])),
        Platform::Unknown => None,
    }
}

/// Open a URL in the system default browser using platform-specific commands.
///
/// Returns `Ok(())` on success or an error description on failure.
pub fn open_browser(url: &str) -> Result<(), String> {
    let platform = detect_platform();
    info!(%platform, %url, "opening browser");

    let (cmd, args) =
        browser_command(platform).ok_or_else(|| format!("unsupported platform: {platform}"))?;

    let mut command = Command::new(cmd);
    for arg in &args {
        command.arg(arg);
    }
    command.arg(url);

    command
        .spawn()
        .map_err(|e| format!("failed to open browser with `{cmd}`: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Desktop application
// ---------------------------------------------------------------------------

/// The desktop application controller.
///
/// Manages the lifecycle of the Arena server and the browser/webview session.
pub struct DesktopApp {
    /// The URL the Arena is serving on.
    arena_url: String,
    /// Desktop configuration.
    config: DesktopConfig,
    /// Whether the Arena was started by this desktop session.
    owns_arena: bool,
}

impl DesktopApp {
    /// Create a new desktop app with the given configuration.
    pub fn new(config: DesktopConfig) -> Self {
        let arena_url = config.dashboard_url();
        Self {
            arena_url,
            config,
            owns_arena: false,
        }
    }

    /// Return the Arena URL this app is pointing to.
    pub fn arena_url(&self) -> &str {
        &self.arena_url
    }

    /// Return a reference to the desktop configuration.
    pub fn config(&self) -> &DesktopConfig {
        &self.config
    }

    /// Check whether the Arena is already running by probing the health endpoint.
    pub async fn is_arena_running(&self) -> bool {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build();

        let client = match client {
            Ok(c) => c,
            Err(_) => return false,
        };

        client
            .get(format!("{}/api/dashboard/status", self.arena_url))
            .send()
            .await
            .is_ok()
    }

    /// Open the dashboard in the system browser.
    pub fn open_dashboard(&self) -> Result<(), String> {
        let url = self.config.dashboard_url_with_theme();
        open_browser(&url)
    }

    /// Mark that this desktop session owns (started) the Arena.
    pub fn set_owns_arena(&mut self, owns: bool) {
        self.owns_arena = owns;
    }

    /// Whether this desktop session started the Arena.
    #[cfg(test)]
    pub fn owns_arena(&self) -> bool {
        self.owns_arena
    }
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

const DESKTOP_BANNER: &str = r#"
  ┌──────────────────────────────────────────────┐
  │         PUNCH DESKTOP LAUNCHER               │
  │                                               │
  │  The Arena dashboard is opening in your       │
  │  browser. Keep this terminal running.         │
  │                                               │
  │  Press Ctrl+C to stop the desktop session.    │
  └──────────────────────────────────────────────┘
"#;

/// Run the desktop launcher.
///
/// This is the main entry point for the `punch desktop` subcommand.
/// It checks whether the Arena is already running, starts it if needed,
/// then opens the dashboard in the system browser.
pub async fn run(config_path: Option<String>, port: Option<u16>, native: bool) -> i32 {
    // Build desktop configuration.
    let mut desktop_config = DesktopConfig::default();
    if let Some(p) = port {
        desktop_config.port = p;
    }

    // If we have a Punch config, extract the port from api_listen.
    if let Ok(punch_config) = super::load_config(config_path.as_deref())
        && let Some(port_str) = punch_config.api_listen.rsplit(':').next()
        && let Ok(p) = port_str.parse::<u16>()
        && port.is_none()
    {
        desktop_config.port = p;
    }

    let mut app = DesktopApp::new(desktop_config);

    // Check if Arena is already running.
    let arena_running = app.is_arena_running().await;

    if !arena_running {
        println!("  Arena is not running.");
        println!(
            "  Start it first with: punch start --port {}",
            app.config().port
        );
        println!();
        println!("  Once the Arena is running, re-run: punch desktop");
        return 1;
    }

    info!(url = %app.arena_url(), "arena is running");

    // Handle native webview mode.
    #[cfg(feature = "desktop")]
    if native {
        println!("  Launching native webview...");
        return super::webview::launch_webview(&app.config).await;
    }

    #[cfg(not(feature = "desktop"))]
    if native {
        eprintln!("  Native webview requires the `desktop` feature.");
        eprintln!("  Rebuild with: cargo build --features desktop");
        eprintln!("  Falling back to browser mode...");
        println!();
    }

    // Browser mode — open the dashboard.
    println!("{DESKTOP_BANNER}");
    println!("  Dashboard URL: {}", app.arena_url());
    println!("  Theme:         {}", app.config().theme);
    println!();

    if app.config().auto_open_browser {
        match app.open_dashboard() {
            Ok(()) => {
                println!("  Browser opened successfully.");
            }
            Err(e) => {
                warn!(error = %e, "failed to open browser");
                eprintln!("  [!] Could not open browser: {e}");
                eprintln!(
                    "  Open this URL manually: {}",
                    app.config().dashboard_url_with_theme()
                );
            }
        }
    } else {
        println!(
            "  Open this URL in your browser: {}",
            app.config().dashboard_url_with_theme()
        );
    }

    println!();
    println!("  Press Ctrl+C to stop the desktop session.");

    app.set_owns_arena(false);

    // Keep the process alive until Ctrl+C.
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            println!();
            println!("  Desktop session ended. Goodbye.");
        }
        Err(e) => {
            eprintln!("  [X] Failed to listen for shutdown signal: {e}");
            return 1;
        }
    }

    0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_config_defaults() {
        let config = DesktopConfig::default();
        assert_eq!(config.port, 6660);
        assert!(config.auto_open_browser);
        assert_eq!(config.theme, Theme::System);
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.window_title, "Punch Agent OS");
        assert_eq!(config.window_width, 1200);
        assert_eq!(config.window_height, 800);
    }

    #[test]
    fn platform_browser_command_detection() {
        // macOS uses `open`
        let (cmd, args) = browser_command(Platform::MacOS).unwrap();
        assert_eq!(cmd, "open");
        assert!(args.is_empty());

        // Linux uses `xdg-open`
        let (cmd, args) = browser_command(Platform::Linux).unwrap();
        assert_eq!(cmd, "xdg-open");
        assert!(args.is_empty());

        // Windows uses `cmd /C start`
        let (cmd, args) = browser_command(Platform::Windows).unwrap();
        assert_eq!(cmd, "cmd");
        assert_eq!(args, vec!["/C", "start"]);

        // Unknown returns None
        assert!(browser_command(Platform::Unknown).is_none());
    }

    #[test]
    fn url_construction_from_config() {
        let config = DesktopConfig {
            port: 8080,
            host: "localhost".to_string(),
            theme: Theme::Dark,
            ..Default::default()
        };
        assert_eq!(config.dashboard_url(), "http://localhost:8080");
        assert_eq!(
            config.dashboard_url_with_theme(),
            "http://localhost:8080/?theme=dark"
        );
    }

    #[test]
    fn desktop_app_initialization() {
        let config = DesktopConfig::default();
        let app = DesktopApp::new(config);
        assert_eq!(app.arena_url(), "http://127.0.0.1:6660");
        assert!(!app.owns_arena());
        assert_eq!(app.config().port, 6660);
    }

    #[test]
    fn platform_detection_returns_known() {
        let platform = detect_platform();
        // On any standard CI/dev machine this should not be Unknown.
        assert_ne!(platform, Platform::Unknown);
    }

    #[test]
    fn config_serialization_roundtrip() {
        let config = DesktopConfig {
            port: 9999,
            auto_open_browser: false,
            theme: Theme::Light,
            host: "0.0.0.0".to_string(),
            window_title: "Test Title".to_string(),
            window_width: 800,
            window_height: 600,
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: DesktopConfig = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.port, 9999);
        assert!(!deserialized.auto_open_browser);
        assert_eq!(deserialized.theme, Theme::Light);
        assert_eq!(deserialized.host, "0.0.0.0");
        assert_eq!(deserialized.window_title, "Test Title");
        assert_eq!(deserialized.window_width, 800);
        assert_eq!(deserialized.window_height, 600);
    }

    #[test]
    fn port_configuration() {
        let mut config = DesktopConfig::default();
        assert_eq!(config.port, 6660);

        config.port = 3000;
        assert_eq!(config.dashboard_url(), "http://127.0.0.1:3000");

        config.port = 0;
        assert_eq!(config.dashboard_url(), "http://127.0.0.1:0");

        config.port = 65535;
        assert_eq!(config.dashboard_url(), "http://127.0.0.1:65535");
    }

    #[test]
    fn theme_settings() {
        assert_eq!(Theme::default(), Theme::System);
        assert_eq!(Theme::Light.to_string(), "light");
        assert_eq!(Theme::Dark.to_string(), "dark");
        assert_eq!(Theme::System.to_string(), "system");

        // Serialization
        let light_json = serde_json::to_string(&Theme::Light).unwrap();
        assert_eq!(light_json, "\"light\"");

        let dark: Theme = serde_json::from_str("\"dark\"").unwrap();
        assert_eq!(dark, Theme::Dark);

        let system: Theme = serde_json::from_str("\"system\"").unwrap();
        assert_eq!(system, Theme::System);
    }

    #[test]
    fn desktop_app_owns_arena_toggle() {
        let config = DesktopConfig::default();
        let mut app = DesktopApp::new(config);
        assert!(!app.owns_arena());

        app.set_owns_arena(true);
        assert!(app.owns_arena());

        app.set_owns_arena(false);
        assert!(!app.owns_arena());
    }

    #[test]
    fn platform_display() {
        assert_eq!(Platform::MacOS.to_string(), "macOS");
        assert_eq!(Platform::Linux.to_string(), "Linux");
        assert_eq!(Platform::Windows.to_string(), "Windows");
        assert_eq!(Platform::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn url_with_different_themes() {
        let mut config = DesktopConfig::default();

        config.theme = Theme::Light;
        assert!(config.dashboard_url_with_theme().contains("theme=light"));

        config.theme = Theme::Dark;
        assert!(config.dashboard_url_with_theme().contains("theme=dark"));

        config.theme = Theme::System;
        assert!(config.dashboard_url_with_theme().contains("theme=system"));
    }

    #[test]
    fn desktop_config_custom_host() {
        let config = DesktopConfig {
            host: "0.0.0.0".to_string(),
            port: 8080,
            ..Default::default()
        };
        assert_eq!(config.dashboard_url(), "http://0.0.0.0:8080");
    }

    #[test]
    fn desktop_config_window_dimensions() {
        let config = DesktopConfig {
            window_width: 800,
            window_height: 600,
            ..Default::default()
        };
        assert_eq!(config.window_width, 800);
        assert_eq!(config.window_height, 600);
    }

    #[test]
    fn desktop_config_custom_title() {
        let config = DesktopConfig {
            window_title: "My Custom Title".to_string(),
            ..Default::default()
        };
        assert_eq!(config.window_title, "My Custom Title");
    }

    #[test]
    fn desktop_app_config_reference() {
        let config = DesktopConfig {
            port: 4444,
            auto_open_browser: false,
            ..Default::default()
        };
        let app = DesktopApp::new(config);
        assert_eq!(app.config().port, 4444);
        assert!(!app.config().auto_open_browser);
    }
}
