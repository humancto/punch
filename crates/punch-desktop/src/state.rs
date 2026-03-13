//! Application state management for the desktop wrapper.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Desktop application state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopState {
    /// URL of the Arena API server.
    pub arena_url: String,
    /// Whether the Arena server has been started.
    pub arena_running: bool,
    /// Whether the desktop app is connected to the Arena.
    pub connected: bool,
    /// When the desktop app was started.
    pub started_at: DateTime<Utc>,
    /// Current theme preference.
    pub theme: Theme,
    /// Whether to auto-open the browser on startup.
    pub auto_open_browser: bool,
}

impl DesktopState {
    /// Create a new desktop state with the given Arena URL.
    pub fn new(arena_url: String) -> Self {
        Self {
            arena_url,
            arena_running: false,
            connected: false,
            started_at: Utc::now(),
            theme: Theme::Dark,
            auto_open_browser: true,
        }
    }

    /// Mark the Arena as running and connected.
    pub fn mark_connected(&mut self) {
        self.arena_running = true;
        self.connected = true;
    }

    /// Mark the Arena as disconnected.
    pub fn mark_disconnected(&mut self) {
        self.connected = false;
    }

    /// Get the uptime in seconds since the desktop app started.
    pub fn uptime_secs(&self) -> i64 {
        Utc::now()
            .signed_duration_since(self.started_at)
            .num_seconds()
    }

    /// Get the full dashboard URL.
    pub fn dashboard_url(&self) -> String {
        format!("{}/dashboard", self.arena_url)
    }

    /// Get the API health endpoint URL.
    pub fn health_url(&self) -> String {
        format!("{}/health", self.arena_url)
    }

    /// Get the API status endpoint URL.
    pub fn status_url(&self) -> String {
        format!("{}/api/status", self.arena_url)
    }
}

/// Theme selection for the desktop UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Dark theme (default).
    Dark,
    /// Light theme.
    Light,
    /// Follow system preference.
    System,
}

impl std::fmt::Display for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dark => write!(f, "dark"),
            Self::Light => write!(f, "light"),
            Self::System => write!(f, "system"),
        }
    }
}

impl std::str::FromStr for Theme {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            "system" => Ok(Self::System),
            other => Err(format!("unknown theme: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state_defaults() {
        let state = DesktopState::new("http://localhost:6660".to_string());
        assert_eq!(state.arena_url, "http://localhost:6660");
        assert!(!state.arena_running);
        assert!(!state.connected);
        assert_eq!(state.theme, Theme::Dark);
        assert!(state.auto_open_browser);
    }

    #[test]
    fn test_mark_connected() {
        let mut state = DesktopState::new("http://localhost:6660".to_string());
        state.mark_connected();
        assert!(state.arena_running);
        assert!(state.connected);
    }

    #[test]
    fn test_mark_disconnected() {
        let mut state = DesktopState::new("http://localhost:6660".to_string());
        state.mark_connected();
        state.mark_disconnected();
        assert!(!state.connected);
        // arena_running stays true — the process may still be alive.
        assert!(state.arena_running);
    }

    #[test]
    fn test_uptime_is_non_negative() {
        let state = DesktopState::new("http://localhost:6660".to_string());
        assert!(state.uptime_secs() >= 0);
    }

    #[test]
    fn test_dashboard_url() {
        let state = DesktopState::new("http://localhost:6660".to_string());
        assert_eq!(state.dashboard_url(), "http://localhost:6660/dashboard");
    }

    #[test]
    fn test_health_url() {
        let state = DesktopState::new("http://localhost:6660".to_string());
        assert_eq!(state.health_url(), "http://localhost:6660/health");
    }

    #[test]
    fn test_status_url() {
        let state = DesktopState::new("http://localhost:6660".to_string());
        assert_eq!(state.status_url(), "http://localhost:6660/api/status");
    }

    #[test]
    fn test_theme_display() {
        assert_eq!(Theme::Dark.to_string(), "dark");
        assert_eq!(Theme::Light.to_string(), "light");
        assert_eq!(Theme::System.to_string(), "system");
    }

    #[test]
    fn test_theme_from_str() {
        assert_eq!("dark".parse::<Theme>().unwrap(), Theme::Dark);
        assert_eq!("LIGHT".parse::<Theme>().unwrap(), Theme::Light);
        assert_eq!("System".parse::<Theme>().unwrap(), Theme::System);
        assert!("invalid".parse::<Theme>().is_err());
    }

    #[test]
    fn test_state_serialization() {
        let state = DesktopState::new("http://localhost:6660".to_string());
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: DesktopState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.arena_url, state.arena_url);
        assert_eq!(deserialized.theme, state.theme);
    }

    #[test]
    fn test_state_connection_lifecycle() {
        let mut state = DesktopState::new("http://localhost:6660".to_string());

        // Initial state
        assert!(!state.arena_running);
        assert!(!state.connected);

        // Connect
        state.mark_connected();
        assert!(state.arena_running);
        assert!(state.connected);

        // Disconnect
        state.mark_disconnected();
        assert!(state.arena_running); // still running
        assert!(!state.connected);

        // Reconnect
        state.mark_connected();
        assert!(state.connected);
    }

    #[test]
    fn test_theme_case_insensitivity() {
        assert_eq!("DARK".parse::<Theme>().unwrap(), Theme::Dark);
        assert_eq!("Dark".parse::<Theme>().unwrap(), Theme::Dark);
        assert_eq!("dark".parse::<Theme>().unwrap(), Theme::Dark);
        assert_eq!("LIGHT".parse::<Theme>().unwrap(), Theme::Light);
        assert_eq!("Light".parse::<Theme>().unwrap(), Theme::Light);
        assert_eq!("SYSTEM".parse::<Theme>().unwrap(), Theme::System);
    }

    #[test]
    fn test_theme_error_message() {
        let result = "purple".parse::<Theme>();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unknown theme"));
        assert!(err.contains("purple"));
    }

    #[test]
    fn test_dashboard_url_custom_port() {
        let state = DesktopState::new("http://localhost:9999".to_string());
        assert_eq!(state.dashboard_url(), "http://localhost:9999/dashboard");
    }

    #[test]
    fn test_health_url_format() {
        let state = DesktopState::new("http://192.168.1.100:3000".to_string());
        assert_eq!(state.health_url(), "http://192.168.1.100:3000/health");
    }

    #[test]
    fn test_status_url_format() {
        let state = DesktopState::new("http://myhost:8080".to_string());
        assert_eq!(state.status_url(), "http://myhost:8080/api/status");
    }

    #[test]
    fn test_state_theme_switching() {
        let mut state = DesktopState::new("http://localhost:6660".to_string());
        assert_eq!(state.theme, Theme::Dark);

        state.theme = Theme::Light;
        assert_eq!(state.theme, Theme::Light);

        state.theme = Theme::System;
        assert_eq!(state.theme, Theme::System);
    }

    #[test]
    fn test_auto_open_browser_default() {
        let state = DesktopState::new("http://localhost:6660".to_string());
        assert!(state.auto_open_browser);
    }

    #[test]
    fn test_state_deserialization_from_json() {
        let json = r#"{
            "arena_url": "http://localhost:7777",
            "arena_running": true,
            "connected": true,
            "started_at": "2024-01-01T00:00:00Z",
            "theme": "light",
            "auto_open_browser": false
        }"#;
        let state: DesktopState = serde_json::from_str(json).unwrap();
        assert_eq!(state.arena_url, "http://localhost:7777");
        assert!(state.arena_running);
        assert!(state.connected);
        assert_eq!(state.theme, Theme::Light);
        assert!(!state.auto_open_browser);
    }

    #[test]
    fn test_theme_serialization_roundtrip() {
        for theme in [Theme::Dark, Theme::Light, Theme::System] {
            let json = serde_json::to_string(&theme).unwrap();
            let back: Theme = serde_json::from_str(&json).unwrap();
            assert_eq!(theme, back);
        }
    }
}
