//! Shared types for the automation subsystem.
//!
//! These types are used across all platform-specific backends to provide
//! a unified view of running applications, windows, UI elements, and
//! clipboard content.

use serde::{Deserialize, Serialize};

/// Information about a running application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    /// Application name as shown in the OS.
    pub name: String,
    /// Process ID.
    pub pid: u32,
    /// Whether the application is currently focused/frontmost.
    pub is_frontmost: bool,
}

/// Information about a window on screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Window title.
    pub title: String,
    /// The application that owns this window.
    pub app_name: String,
    /// Window position (x, y) from top-left of screen.
    pub position: Option<(i32, i32)>,
    /// Window size (width, height).
    pub size: Option<(u32, u32)>,
    /// Whether the window is minimized.
    pub is_minimized: bool,
}

/// A UI element found via accessibility APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiElement {
    /// Opaque identifier for this element in the current session.
    pub element_id: String,
    /// Accessibility role (e.g. "button", "text_field", "menu_item").
    pub role: String,
    /// Human-readable label or title.
    pub label: Option<String>,
    /// Current value (for text fields, sliders, etc.).
    pub value: Option<String>,
    /// Whether the element is currently enabled.
    pub enabled: bool,
}

/// Selector for finding UI elements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSelector {
    /// Filter by accessibility role (e.g. "button", "text_field").
    pub role: Option<String>,
    /// Filter by label/title (partial match).
    pub label: Option<String>,
    /// Filter by value (partial match).
    pub value: Option<String>,
}

/// Content read from or written to the clipboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardContent {
    /// The text content of the clipboard.
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_info_serde_roundtrip() {
        let info = AppInfo {
            name: "Safari".to_string(),
            pid: 1234,
            is_frontmost: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        let deser: AppInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.name, "Safari");
        assert_eq!(deser.pid, 1234);
        assert!(deser.is_frontmost);
    }

    #[test]
    fn test_window_info_serde_roundtrip() {
        let info = WindowInfo {
            title: "My Document".to_string(),
            app_name: "TextEdit".to_string(),
            position: Some((100, 200)),
            size: Some((800, 600)),
            is_minimized: false,
        };
        let json = serde_json::to_string(&info).unwrap();
        let deser: WindowInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.title, "My Document");
        assert_eq!(deser.app_name, "TextEdit");
        assert_eq!(deser.position, Some((100, 200)));
        assert_eq!(deser.size, Some((800, 600)));
        assert!(!deser.is_minimized);
    }

    #[test]
    fn test_window_info_optional_fields() {
        let info = WindowInfo {
            title: "Untitled".to_string(),
            app_name: "App".to_string(),
            position: None,
            size: None,
            is_minimized: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        let deser: WindowInfo = serde_json::from_str(&json).unwrap();
        assert!(deser.position.is_none());
        assert!(deser.size.is_none());
        assert!(deser.is_minimized);
    }

    #[test]
    fn test_ui_element_serde_roundtrip() {
        let elem = UiElement {
            element_id: "btn-001".to_string(),
            role: "button".to_string(),
            label: Some("OK".to_string()),
            value: None,
            enabled: true,
        };
        let json = serde_json::to_string(&elem).unwrap();
        let deser: UiElement = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.element_id, "btn-001");
        assert_eq!(deser.role, "button");
        assert_eq!(deser.label.as_deref(), Some("OK"));
        assert!(deser.value.is_none());
        assert!(deser.enabled);
    }

    #[test]
    fn test_ui_selector_serde_roundtrip() {
        let sel = UiSelector {
            role: Some("button".to_string()),
            label: Some("Submit".to_string()),
            value: None,
        };
        let json = serde_json::to_string(&sel).unwrap();
        let deser: UiSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.role.as_deref(), Some("button"));
        assert_eq!(deser.label.as_deref(), Some("Submit"));
        assert!(deser.value.is_none());
    }

    #[test]
    fn test_ui_selector_empty() {
        let sel = UiSelector {
            role: None,
            label: None,
            value: None,
        };
        let json = serde_json::to_string(&sel).unwrap();
        let deser: UiSelector = serde_json::from_str(&json).unwrap();
        assert!(deser.role.is_none());
        assert!(deser.label.is_none());
        assert!(deser.value.is_none());
    }

    #[test]
    fn test_clipboard_content_serde_roundtrip() {
        let clip = ClipboardContent {
            text: "Hello, world!".to_string(),
        };
        let json = serde_json::to_string(&clip).unwrap();
        let deser: ClipboardContent = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.text, "Hello, world!");
    }

    #[test]
    fn test_clipboard_content_empty() {
        let clip = ClipboardContent {
            text: String::new(),
        };
        let json = serde_json::to_string(&clip).unwrap();
        let deser: ClipboardContent = serde_json::from_str(&json).unwrap();
        assert!(deser.text.is_empty());
    }
}
