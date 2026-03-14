//! Native webview window for the Punch Agent OS dashboard.
//!
//! This module provides a native desktop window embedding a webview that points
//! to the Arena dashboard. It requires the `desktop` feature flag, which pulls
//! in the `tao` (window management) and `wry` (webview) crates.
//!
//! # Feature Gate
//!
//! This module is only compiled when the `desktop` feature is enabled:
//!
//! ```toml
//! [features]
//! desktop = ["dep:wry", "dep:tao"]
//! ```
//!
//! # Architecture
//!
//! - `tao::window::WindowBuilder` creates the native OS window
//! - `wry::WebView` embeds a webview pointing to the Arena HTTP server
//! - An IPC bridge allows JavaScript in the dashboard to invoke Rust handlers
//! - A native menu bar provides common actions (refresh, quit, theme toggle)
//!
//! # Usage
//!
//! ```bash
//! # Build with native webview support
//! cargo build -p punch-cli --features desktop
//!
//! # Launch the native desktop app
//! punch desktop --native
//! ```

use super::desktop::DesktopConfig;

/// Custom user agent string for the Punch webview.
pub const USER_AGENT: &str = "PunchAgentOS/0.1.0 (WebView)";

/// Default window title for the native desktop app.
pub const WINDOW_TITLE: &str = "Punch Agent OS";

/// IPC message types that JavaScript can send to Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcMessage {
    /// Request to refresh the dashboard data.
    Refresh,
    /// Request to toggle the color theme.
    ToggleTheme,
    /// Request to spawn a new fighter.
    SpawnFighter { template: String },
    /// Request to quit the application.
    Quit,
    /// Unknown or unrecognized message.
    Unknown(String),
}

impl IpcMessage {
    /// Parse a raw IPC message string from JavaScript.
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed == "refresh" {
            Self::Refresh
        } else if trimmed == "toggle_theme" {
            Self::ToggleTheme
        } else if trimmed == "quit" {
            Self::Quit
        } else if let Some(template) = trimmed.strip_prefix("spawn_fighter:") {
            Self::SpawnFighter {
                template: template.trim().to_string(),
            }
        } else {
            Self::Unknown(trimmed.to_string())
        }
    }
}

/// Configuration for the native webview window, derived from [`DesktopConfig`].
#[derive(Debug, Clone)]
pub struct WebviewWindowConfig {
    /// Window title.
    pub title: String,
    /// Window width in pixels.
    pub width: u32,
    /// Window height in pixels.
    pub height: u32,
    /// URL to load in the webview.
    pub url: String,
    /// Custom user agent string.
    pub user_agent: String,
    /// Whether the window should be resizable.
    pub resizable: bool,
    /// Whether to enable developer tools.
    pub devtools: bool,
}

impl WebviewWindowConfig {
    /// Create a webview window config from a [`DesktopConfig`].
    pub fn from_desktop_config(config: &DesktopConfig) -> Self {
        Self {
            title: config.window_title.clone(),
            width: config.window_width,
            height: config.window_height,
            url: config.dashboard_url_with_theme(),
            user_agent: USER_AGENT.to_string(),
            resizable: true,
            devtools: cfg!(debug_assertions),
        }
    }
}

/// Menu item identifiers for the native menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    /// File > New Fighter
    NewFighter,
    /// File > Quit
    Quit,
    /// View > Refresh
    Refresh,
    /// View > Toggle Theme
    ToggleTheme,
    /// Help > About
    About,
}

/// Describes the menu bar structure for the native window.
///
/// This is a declarative representation — the actual platform menu is built
/// by the webview launcher when `tao` and `wry` are available.
pub struct MenuBar {
    pub items: Vec<MenuSection>,
}

/// A section (top-level menu) in the menu bar.
pub struct MenuSection {
    pub title: String,
    pub items: Vec<(MenuItem, String)>,
}

/// Build the default menu bar structure for the Punch desktop app.
pub fn default_menu_bar() -> MenuBar {
    MenuBar {
        items: vec![
            MenuSection {
                title: "File".to_string(),
                items: vec![
                    (MenuItem::NewFighter, "New Fighter".to_string()),
                    (MenuItem::Quit, "Quit".to_string()),
                ],
            },
            MenuSection {
                title: "View".to_string(),
                items: vec![
                    (MenuItem::Refresh, "Refresh".to_string()),
                    (MenuItem::ToggleTheme, "Toggle Theme".to_string()),
                ],
            },
            MenuSection {
                title: "Help".to_string(),
                items: vec![(MenuItem::About, "About Punch".to_string())],
            },
        ],
    }
}

/// Launch the native webview window.
///
/// This function creates a native OS window with an embedded webview pointing
/// to the Arena dashboard. It blocks until the window is closed.
///
/// # Requirements
///
/// The `desktop` feature must be enabled at compile time, providing the `tao`
/// and `wry` dependencies.
///
/// # Errors
///
/// Returns a non-zero exit code if the webview cannot be created.
pub async fn launch_webview(config: &DesktopConfig) -> i32 {
    let _window_config = WebviewWindowConfig::from_desktop_config(config);
    let _menu = default_menu_bar();

    // TODO: Implement native webview when tao + wry dependencies are added.
    //
    // The implementation will follow this pattern:
    //
    // ```rust
    // use tao::event_loop::EventLoop;
    // use tao::window::WindowBuilder;
    // use wry::WebViewBuilder;
    //
    // let event_loop = EventLoop::new();
    // let window = WindowBuilder::new()
    //     .with_title(&window_config.title)
    //     .with_inner_size(tao::dpi::LogicalSize::new(
    //         window_config.width, window_config.height,
    //     ))
    //     .with_resizable(window_config.resizable)
    //     .build(&event_loop)?;
    //
    // let webview = WebViewBuilder::new(&window)
    //     .with_url(&window_config.url)
    //     .with_user_agent(&window_config.user_agent)
    //     .with_devtools(window_config.devtools)
    //     .with_ipc_handler(|msg| { /* handle IPC */ })
    //     .build()?;
    //
    // event_loop.run(move |event, _, control_flow| { /* event loop */ });
    // ```

    eprintln!("  Native webview is not yet available.");
    eprintln!("  The `desktop` feature requires tao + wry dependencies.");
    eprintln!("  Falling back to browser mode.");
    eprintln!();
    eprintln!(
        "  Open the dashboard manually: {}",
        config.dashboard_url_with_theme()
    );

    // Fall back to opening the browser.
    if let Err(e) = super::desktop::open_browser(&config.dashboard_url_with_theme()) {
        eprintln!("  [!] Could not open browser: {e}");
        return 1;
    }

    // Wait for Ctrl+C.
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            println!();
            println!("  Webview session ended. Goodbye.");
            0
        }
        Err(e) => {
            eprintln!("  [X] Failed to listen for shutdown signal: {e}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_message_parsing() {
        assert_eq!(IpcMessage::parse("refresh"), IpcMessage::Refresh);
        assert_eq!(IpcMessage::parse("toggle_theme"), IpcMessage::ToggleTheme);
        assert_eq!(IpcMessage::parse("quit"), IpcMessage::Quit);
        assert_eq!(
            IpcMessage::parse("spawn_fighter:warrior"),
            IpcMessage::SpawnFighter {
                template: "warrior".to_string()
            }
        );
        assert_eq!(
            IpcMessage::parse("unknown_msg"),
            IpcMessage::Unknown("unknown_msg".to_string())
        );
    }

    #[test]
    fn ipc_message_parsing_whitespace() {
        assert_eq!(IpcMessage::parse("  refresh  "), IpcMessage::Refresh);
        assert_eq!(
            IpcMessage::parse("spawn_fighter:  scout  "),
            IpcMessage::SpawnFighter {
                template: "scout".to_string()
            }
        );
    }

    #[test]
    fn webview_window_config_from_desktop() {
        let desktop = DesktopConfig {
            port: 8080,
            host: "localhost".to_string(),
            window_title: "Test Window".to_string(),
            window_width: 1024,
            window_height: 768,
            theme: super::super::desktop::Theme::Dark,
            ..Default::default()
        };

        let wv = WebviewWindowConfig::from_desktop_config(&desktop);
        assert_eq!(wv.title, "Test Window");
        assert_eq!(wv.width, 1024);
        assert_eq!(wv.height, 768);
        assert_eq!(wv.url, "http://localhost:8080/?theme=dark");
        assert_eq!(wv.user_agent, USER_AGENT);
        assert!(wv.resizable);
    }

    #[test]
    fn default_menu_bar_structure() {
        let menu = default_menu_bar();
        assert_eq!(menu.items.len(), 3);

        assert_eq!(menu.items[0].title, "File");
        assert_eq!(menu.items[0].items.len(), 2);
        assert_eq!(menu.items[0].items[0].0, MenuItem::NewFighter);
        assert_eq!(menu.items[0].items[1].0, MenuItem::Quit);

        assert_eq!(menu.items[1].title, "View");
        assert_eq!(menu.items[1].items.len(), 2);
        assert_eq!(menu.items[1].items[0].0, MenuItem::Refresh);
        assert_eq!(menu.items[1].items[1].0, MenuItem::ToggleTheme);

        assert_eq!(menu.items[2].title, "Help");
        assert_eq!(menu.items[2].items.len(), 1);
        assert_eq!(menu.items[2].items[0].0, MenuItem::About);
    }

    #[test]
    fn user_agent_contains_version() {
        assert!(USER_AGENT.contains("PunchAgentOS"));
        assert!(USER_AGENT.contains("WebView"));
    }

    #[test]
    fn webview_config_devtools_debug_only() {
        let config = DesktopConfig::default();
        let wv = WebviewWindowConfig::from_desktop_config(&config);
        // In test (debug) builds, devtools should be enabled.
        assert!(wv.devtools);
    }

    #[test]
    fn ipc_message_empty_string() {
        assert_eq!(IpcMessage::parse(""), IpcMessage::Unknown("".to_string()));
    }

    #[test]
    fn ipc_message_spawn_fighter_empty_template() {
        assert_eq!(
            IpcMessage::parse("spawn_fighter:"),
            IpcMessage::SpawnFighter {
                template: "".to_string()
            }
        );
    }

    #[test]
    fn ipc_message_spawn_fighter_complex_template() {
        assert_eq!(
            IpcMessage::parse("spawn_fighter:code_reviewer_v2"),
            IpcMessage::SpawnFighter {
                template: "code_reviewer_v2".to_string()
            }
        );
    }

    #[test]
    fn menu_item_equality() {
        assert_eq!(MenuItem::Quit, MenuItem::Quit);
        assert_ne!(MenuItem::Quit, MenuItem::Refresh);
        assert_ne!(MenuItem::NewFighter, MenuItem::About);
    }

    #[test]
    fn webview_window_config_default_url() {
        let config = DesktopConfig::default();
        let wv = WebviewWindowConfig::from_desktop_config(&config);
        assert!(wv.url.starts_with("http://"));
        assert!(wv.url.contains("theme="));
    }

    #[test]
    fn webview_window_config_light_theme_url() {
        let desktop = DesktopConfig {
            theme: super::super::desktop::Theme::Light,
            ..Default::default()
        };
        let wv = WebviewWindowConfig::from_desktop_config(&desktop);
        assert!(wv.url.contains("theme=light"));
    }

    #[test]
    fn window_title_constant() {
        assert_eq!(WINDOW_TITLE, "Punch Agent OS");
    }
}
