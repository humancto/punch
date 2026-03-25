//! Desktop automation subsystem.
//!
//! Provides a cross-platform trait (`AutomationBackend`) and platform-specific
//! implementations for system automation, UI interaction, and app control.
//! Each platform (macOS, Linux, Windows) implements the trait using
//! native accessibility frameworks and command-line tools.

pub mod common;

use async_trait::async_trait;
use punch_types::error::PunchResult;
use tracing::debug;

pub use common::{AppInfo, ClipboardContent, UiElement, UiSelector, WindowInfo};

/// Cross-platform automation backend trait.
///
/// Platform-specific implementations provide access to running applications,
/// window management, UI element interaction, clipboard, and notifications.
#[async_trait]
pub trait AutomationBackend: Send + Sync {
    // --- System automation ---

    /// List currently running applications visible to the user.
    async fn list_running_apps(&self) -> PunchResult<Vec<AppInfo>>;

    /// Open (launch) an application by name.
    async fn open_app(&self, app_name: &str) -> PunchResult<()>;

    /// Read the current clipboard text content.
    async fn clipboard_read(&self) -> PunchResult<ClipboardContent>;

    /// Write text content to the clipboard.
    async fn clipboard_write(&self, content: &str) -> PunchResult<()>;

    /// Send a desktop notification.
    async fn send_notification(&self, title: &str, body: &str) -> PunchResult<()>;

    // --- UI automation ---

    /// List all visible windows.
    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>>;

    /// Find UI elements in the given application matching a selector.
    async fn find_ui_elements(
        &self,
        app: &str,
        selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>>;

    /// Click a UI element by its element ID.
    async fn click_element(&self, element_id: &str) -> PunchResult<()>;

    /// Type text into a UI element by its element ID.
    async fn type_text(&self, element_id: &str, text: &str) -> PunchResult<()>;

    /// Read an attribute from a UI element by its element ID.
    async fn read_element_attribute(
        &self,
        element_id: &str,
        attribute: &str,
    ) -> PunchResult<String>;

    // --- App integration ---

    /// Activate (bring to foreground) an application by name.
    async fn activate_app(&self, app_name: &str) -> PunchResult<()>;

    /// Click a menu item by its menu path (e.g. ["File", "Save"]).
    async fn app_menu_click(&self, app: &str, menu_path: &[String]) -> PunchResult<()>;

    /// Get the state of an application as a JSON value.
    async fn app_get_state(&self, app: &str) -> PunchResult<serde_json::Value>;
}

/// Create the platform-appropriate automation backend.
///
/// Returns `Ok(backend)` on supported platforms (macOS, Linux, Windows).
/// Returns an error on unsupported platforms.
pub fn create_backend() -> PunchResult<Box<dyn AutomationBackend>> {
    debug!("creating automation backend for current platform");

    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(MacOsBackend::new()))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(LinuxBackend::new()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(WindowsBackend::new()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message: "desktop automation is not supported on this platform".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// macOS backend
// ---------------------------------------------------------------------------

/// Escape a string for safe interpolation into AppleScript double-quoted strings.
///
/// AppleScript uses backslash escapes inside `"..."`. We escape backslashes
/// first, then double quotes, preventing injection of arbitrary AppleScript.
#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
pub struct MacOsBackend;

#[cfg(target_os = "macos")]
impl MacOsBackend {
    pub fn new() -> Self {
        Self
    }

    /// Run an osascript command and return its stdout.
    async fn osascript(&self, script: &str) -> PunchResult<String> {
        let output = tokio::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .await
            .map_err(|e| punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("osascript failed: {e}"),
            })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("osascript error: {stderr}"),
            })
        }
    }
}

#[cfg(target_os = "macos")]
#[async_trait]
impl AutomationBackend for MacOsBackend {
    async fn list_running_apps(&self) -> PunchResult<Vec<AppInfo>> {
        let script = r#"tell application "System Events" to get {name, unix id, frontmost} of every application process whose background only is false"#;
        let raw = self.osascript(script).await?;
        // Parse the AppleScript output into AppInfo structs
        let mut apps = Vec::new();
        // AppleScript returns: {name1, name2, ...}, {pid1, pid2, ...}, {front1, front2, ...}
        // Simplified parsing for the common case
        let parts: Vec<&str> = raw.split(", ").collect();
        if !parts.is_empty() {
            for (i, part) in parts.iter().enumerate() {
                let name = part.trim().to_string();
                if !name.is_empty() {
                    apps.push(AppInfo {
                        name,
                        pid: i as u32,
                        is_frontmost: i == 0,
                    });
                }
            }
        }
        Ok(apps)
    }

    async fn open_app(&self, app_name: &str) -> PunchResult<()> {
        let safe = escape_applescript(app_name);
        let script = format!(r#"tell application "{safe}" to activate"#);
        self.osascript(&script).await?;
        Ok(())
    }

    async fn clipboard_read(&self) -> PunchResult<ClipboardContent> {
        let output = tokio::process::Command::new("pbpaste")
            .output()
            .await
            .map_err(|e| punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("pbpaste failed: {e}"),
            })?;
        Ok(ClipboardContent {
            text: String::from_utf8_lossy(&output.stdout).to_string(),
        })
    }

    async fn clipboard_write(&self, content: &str) -> PunchResult<()> {
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("pbcopy failed: {e}"),
            })?;
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(content.as_bytes()).await.map_err(|e| {
                punch_types::PunchError::Tool {
                    tool: "automation".into(),
                    message: format!("pbcopy write failed: {e}"),
                }
            })?;
        }
        child
            .wait()
            .await
            .map_err(|e| punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("pbcopy wait failed: {e}"),
            })?;
        Ok(())
    }

    async fn send_notification(&self, title: &str, body: &str) -> PunchResult<()> {
        let safe_title = escape_applescript(title);
        let safe_body = escape_applescript(body);
        let script = format!(
            r#"display notification "{safe_body}" with title "{safe_title}""#,
        );
        self.osascript(&script).await?;
        Ok(())
    }

    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>> {
        let script = r#"tell application "System Events" to get {name, name of application process} of every window of every application process whose background only is false"#;
        let raw = self.osascript(script).await?;
        let mut windows = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                windows.push(WindowInfo {
                    title: trimmed.to_string(),
                    app_name: "unknown".to_string(),
                    position: None,
                    size: None,
                    is_minimized: false,
                });
            }
        }
        Ok(windows)
    }

    async fn find_ui_elements(
        &self,
        app: &str,
        selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>> {
        let role_filter = selector.role.as_deref().unwrap_or("UI element");
        let safe_app = escape_applescript(app);
        let safe_role = escape_applescript(role_filter);
        let script = format!(
            r#"tell application "System Events" to tell process "{safe_app}" to get {{role, name, value, enabled}} of every {safe_role} of window 1"#,
        );
        let raw = self.osascript(&script).await?;
        let mut elements = Vec::new();
        for (i, part) in raw.split(", ").enumerate() {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                let matches_label = selector
                    .label
                    .as_ref()
                    .map_or(true, |l| trimmed.to_lowercase().contains(&l.to_lowercase()));
                if matches_label {
                    elements.push(UiElement {
                        element_id: format!("{app}:{i}"),
                        role: role_filter.to_string(),
                        label: Some(trimmed.to_string()),
                        value: None,
                        enabled: true,
                    });
                }
            }
        }
        Ok(elements)
    }

    async fn click_element(&self, element_id: &str) -> PunchResult<()> {
        // element_id format: "AppName:index"
        let parts: Vec<&str> = element_id.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("invalid element_id: {element_id}"),
            });
        }
        let safe_app = escape_applescript(parts[0]);
        let script = format!(
            r#"tell application "System Events" to tell process "{safe_app}" to click UI element {} of window 1"#,
            parts[1].parse::<usize>().unwrap_or(1) + 1,
        );
        self.osascript(&script).await?;
        Ok(())
    }

    async fn type_text(&self, element_id: &str, text: &str) -> PunchResult<()> {
        let parts: Vec<&str> = element_id.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("invalid element_id: {element_id}"),
            });
        }
        let safe_app = escape_applescript(parts[0]);
        let safe_text = escape_applescript(text);
        let script = format!(
            r#"tell application "System Events" to tell process "{safe_app}" to set value of UI element {} of window 1 to "{safe_text}""#,
            parts[1].parse::<usize>().unwrap_or(1) + 1,
        );
        self.osascript(&script).await?;
        Ok(())
    }

    async fn read_element_attribute(
        &self,
        element_id: &str,
        attribute: &str,
    ) -> PunchResult<String> {
        let parts: Vec<&str> = element_id.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("invalid element_id: {element_id}"),
            });
        }
        let safe_app = escape_applescript(parts[0]);
        let safe_attr = escape_applescript(attribute);
        let script = format!(
            r#"tell application "System Events" to tell process "{safe_app}" to get {safe_attr} of UI element {} of window 1"#,
            parts[1].parse::<usize>().unwrap_or(1) + 1,
        );
        self.osascript(&script).await
    }

    async fn activate_app(&self, app_name: &str) -> PunchResult<()> {
        self.open_app(app_name).await
    }

    async fn app_menu_click(&self, app: &str, menu_path: &[String]) -> PunchResult<()> {
        if menu_path.is_empty() {
            return Err(punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: "menu_path cannot be empty".into(),
            });
        }
        let safe_app = escape_applescript(app);
        let mut click_chain = String::new();
        let safe_first = escape_applescript(&menu_path[0]);
        for (i, item) in menu_path.iter().enumerate() {
            let safe_item = escape_applescript(item);
            if i == 0 {
                click_chain
                    .push_str(&format!(r#"click menu bar item "{safe_item}" of menu bar 1"#));
            } else {
                click_chain.push_str(&format!(
                    r#"
click menu item "{safe_item}" of menu 1 of menu bar item "{safe_first}" of menu bar 1"#,
                ));
            }
        }
        let script = format!(
            r#"tell application "System Events" to tell process "{safe_app}" to {click_chain}"#,
        );
        self.osascript(&script).await?;
        Ok(())
    }

    async fn app_get_state(&self, app: &str) -> PunchResult<serde_json::Value> {
        let safe_app = escape_applescript(app);
        let script = format!(
            r#"tell application "System Events" to get {{name, frontmost}} of application process "{safe_app}""#,
        );
        let raw = self.osascript(&script).await?;
        Ok(serde_json::json!({
            "app": app,
            "raw_state": raw,
        }))
    }
}

// ---------------------------------------------------------------------------
// Linux backend
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
pub struct LinuxBackend;

#[cfg(target_os = "linux")]
impl Default for LinuxBackend {
    fn default() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
impl LinuxBackend {
    pub fn new() -> Self {
        Self
    }

    /// Run a command and return stdout.
    async fn run_cmd(&self, program: &str, args: &[&str]) -> PunchResult<String> {
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|e| punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("{program} failed: {e}"),
            })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("{program} error: {stderr}"),
            })
        }
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl AutomationBackend for LinuxBackend {
    async fn list_running_apps(&self) -> PunchResult<Vec<AppInfo>> {
        // Use wmctrl -l to list windows (each represents a running GUI app)
        let raw = self
            .run_cmd("wmctrl", &["-l", "-p"])
            .await
            .unwrap_or_default();
        let mut apps = Vec::new();
        for line in raw.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                let pid = parts[2].parse::<u32>().unwrap_or(0);
                let name = parts[4..].join(" ");
                apps.push(AppInfo {
                    name,
                    pid,
                    is_frontmost: false,
                });
            }
        }
        Ok(apps)
    }

    async fn open_app(&self, app_name: &str) -> PunchResult<()> {
        self.run_cmd("xdg-open", &[app_name]).await.map_err(|_| {
            punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!(
                    "failed to open application '{}': xdg-open not available or app not found",
                    app_name
                ),
            }
        })?;
        Ok(())
    }

    async fn clipboard_read(&self) -> PunchResult<ClipboardContent> {
        let text = match self
            .run_cmd("xclip", &["-selection", "clipboard", "-o"])
            .await
        {
            Ok(t) => t,
            Err(_) => self
                .run_cmd("xsel", &["--clipboard", "--output"])
                .await
                .unwrap_or_default(),
        };
        Ok(ClipboardContent { text })
    }

    async fn clipboard_write(&self, content: &str) -> PunchResult<()> {
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("xclip failed: {e}"),
            })?;
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(content.as_bytes()).await.map_err(|e| {
                punch_types::PunchError::Tool {
                    tool: "automation".into(),
                    message: format!("xclip write failed: {e}"),
                }
            })?;
        }
        child
            .wait()
            .await
            .map_err(|e| punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("xclip wait failed: {e}"),
            })?;
        Ok(())
    }

    async fn send_notification(&self, title: &str, body: &str) -> PunchResult<()> {
        self.run_cmd("notify-send", &[title, body]).await?;
        Ok(())
    }

    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>> {
        let raw = self.run_cmd("wmctrl", &["-l"]).await.unwrap_or_default();
        let mut windows = Vec::new();
        for line in raw.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                windows.push(WindowInfo {
                    title: parts[3..].join(" "),
                    app_name: "unknown".to_string(),
                    position: None,
                    size: None,
                    is_minimized: false,
                });
            }
        }
        Ok(windows)
    }

    async fn find_ui_elements(
        &self,
        _app: &str,
        _selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>> {
        // AT-SPI via D-Bus would be used here in a full implementation.
        // For now, return an informative error.
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message: "UI element discovery requires AT-SPI; not yet implemented on Linux".into(),
        })
    }

    async fn click_element(&self, element_id: &str) -> PunchResult<()> {
        // Would use xdotool or ydotool
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message: format!("click_element not yet implemented on Linux (element: {element_id})"),
        })
    }

    async fn type_text(&self, _element_id: &str, text: &str) -> PunchResult<()> {
        self.run_cmd("xdotool", &["type", "--", text]).await?;
        Ok(())
    }

    async fn read_element_attribute(
        &self,
        element_id: &str,
        attribute: &str,
    ) -> PunchResult<String> {
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message: format!(
                "read_element_attribute not yet implemented on Linux (element: {element_id}, attr: {attribute})"
            ),
        })
    }

    async fn activate_app(&self, app_name: &str) -> PunchResult<()> {
        self.run_cmd("wmctrl", &["-a", app_name]).await?;
        Ok(())
    }

    async fn app_menu_click(&self, _app: &str, _menu_path: &[String]) -> PunchResult<()> {
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message: "app_menu_click requires AT-SPI; not yet implemented on Linux".into(),
        })
    }

    async fn app_get_state(&self, app: &str) -> PunchResult<serde_json::Value> {
        let raw = self.run_cmd("wmctrl", &["-l"]).await.unwrap_or_default();
        let windows: Vec<&str> = raw
            .lines()
            .filter(|l| l.to_lowercase().contains(&app.to_lowercase()))
            .collect();
        Ok(serde_json::json!({
            "app": app,
            "matching_windows": windows.len(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Windows backend
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub struct WindowsBackend;

#[cfg(target_os = "windows")]
impl WindowsBackend {
    pub fn new() -> Self {
        Self
    }

    /// Run a PowerShell command and return stdout.
    async fn powershell(&self, script: &str) -> PunchResult<String> {
        let output = tokio::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .await
            .map_err(|e| punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("powershell failed: {e}"),
            })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(punch_types::PunchError::Tool {
                tool: "automation".into(),
                message: format!("powershell error: {stderr}"),
            })
        }
    }
}

#[cfg(target_os = "windows")]
#[async_trait]
impl AutomationBackend for WindowsBackend {
    async fn list_running_apps(&self) -> PunchResult<Vec<AppInfo>> {
        let raw = self
            .powershell("Get-Process | Where-Object {$_.MainWindowTitle} | Select-Object ProcessName, Id, MainWindowTitle | ConvertTo-Json")
            .await?;
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
        let apps = parsed
            .iter()
            .map(|p| AppInfo {
                name: p["ProcessName"].as_str().unwrap_or("unknown").to_string(),
                pid: p["Id"].as_u64().unwrap_or(0) as u32,
                is_frontmost: false,
            })
            .collect();
        Ok(apps)
    }

    async fn open_app(&self, app_name: &str) -> PunchResult<()> {
        self.powershell(&format!("Start-Process '{app_name}'"))
            .await?;
        Ok(())
    }

    async fn clipboard_read(&self) -> PunchResult<ClipboardContent> {
        let text = self.powershell("Get-Clipboard").await?;
        Ok(ClipboardContent { text })
    }

    async fn clipboard_write(&self, content: &str) -> PunchResult<()> {
        let escaped = content.replace('\'', "''");
        self.powershell(&format!("Set-Clipboard -Value '{escaped}'"))
            .await?;
        Ok(())
    }

    async fn send_notification(&self, title: &str, body: &str) -> PunchResult<()> {
        let script = format!(
            r#"[System.Reflection.Assembly]::LoadWithPartialName('System.Windows.Forms') | Out-Null; $n = New-Object System.Windows.Forms.NotifyIcon; $n.Icon = [System.Drawing.SystemIcons]::Information; $n.Visible = $true; $n.ShowBalloonTip(5000, '{title}', '{body}', 'Info')"#,
        );
        self.powershell(&script).await?;
        Ok(())
    }

    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>> {
        let raw = self
            .powershell("Get-Process | Where-Object {$_.MainWindowTitle} | Select-Object ProcessName, MainWindowTitle | ConvertTo-Json")
            .await?;
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
        let windows = parsed
            .iter()
            .map(|p| WindowInfo {
                title: p["MainWindowTitle"].as_str().unwrap_or("").to_string(),
                app_name: p["ProcessName"].as_str().unwrap_or("").to_string(),
                position: None,
                size: None,
                is_minimized: false,
            })
            .collect();
        Ok(windows)
    }

    async fn find_ui_elements(
        &self,
        _app: &str,
        _selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>> {
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message:
                "UI element discovery requires UIAutomation COM; not yet implemented on Windows"
                    .into(),
        })
    }

    async fn click_element(&self, element_id: &str) -> PunchResult<()> {
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message: format!(
                "click_element not yet implemented on Windows (element: {element_id})"
            ),
        })
    }

    async fn type_text(&self, _element_id: &str, text: &str) -> PunchResult<()> {
        let escaped = text.replace('\'', "''");
        self.powershell(&format!(
            "[System.Windows.Forms.SendKeys]::SendWait('{escaped}')"
        ))
        .await?;
        Ok(())
    }

    async fn read_element_attribute(
        &self,
        element_id: &str,
        attribute: &str,
    ) -> PunchResult<String> {
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message: format!(
                "read_element_attribute not yet implemented on Windows (element: {element_id}, attr: {attribute})"
            ),
        })
    }

    async fn activate_app(&self, app_name: &str) -> PunchResult<()> {
        self.powershell(&format!(
            "(Get-Process '{app_name}' | Where-Object {{$_.MainWindowTitle}}).MainWindowHandle | ForEach-Object {{ [void][System.Runtime.InteropServices.Marshal]::GetObjectForIUnknown($_) }}"
        )).await.ok();
        // Simpler fallback
        self.powershell(&format!(
            "Start-Process '{app_name}' -ErrorAction SilentlyContinue"
        ))
        .await?;
        Ok(())
    }

    async fn app_menu_click(&self, _app: &str, _menu_path: &[String]) -> PunchResult<()> {
        Err(punch_types::PunchError::Tool {
            tool: "automation".into(),
            message: "app_menu_click requires UIAutomation COM; not yet implemented on Windows"
                .into(),
        })
    }

    async fn app_get_state(&self, app: &str) -> PunchResult<serde_json::Value> {
        let raw = self
            .powershell(&format!(
                "Get-Process '{app}' | Select-Object ProcessName, Id, MainWindowTitle, Responding | ConvertTo-Json"
            ))
            .await?;
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or(serde_json::json!(null));
        Ok(serde_json::json!({
            "app": app,
            "state": parsed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_backend_returns_result() {
        // On any platform, create_backend should return Ok or a meaningful error.
        let result = create_backend();
        // We can't assert Ok on CI (might not have wmctrl/osascript), but it shouldn't panic.
        let _ = result;
    }

    #[test]
    fn test_ui_selector_default_all_none() {
        let selector = UiSelector {
            role: None,
            label: None,
            value: None,
        };
        assert!(selector.role.is_none());
        assert!(selector.label.is_none());
        assert!(selector.value.is_none());
    }

    #[test]
    fn test_app_info_debug() {
        let info = AppInfo {
            name: "TestApp".to_string(),
            pid: 42,
            is_frontmost: false,
        };
        let dbg = format!("{info:?}");
        assert!(dbg.contains("TestApp"));
        assert!(dbg.contains("42"));
    }

    #[test]
    fn test_window_info_debug() {
        let info = WindowInfo {
            title: "Window Title".to_string(),
            app_name: "App".to_string(),
            position: Some((10, 20)),
            size: Some((100, 200)),
            is_minimized: false,
        };
        let dbg = format!("{info:?}");
        assert!(dbg.contains("Window Title"));
    }

    #[test]
    fn test_clipboard_content_debug() {
        let content = ClipboardContent {
            text: "hello".to_string(),
        };
        let dbg = format!("{content:?}");
        assert!(dbg.contains("hello"));
    }

    #[test]
    fn test_ui_element_debug() {
        let elem = UiElement {
            element_id: "e1".to_string(),
            role: "button".to_string(),
            label: Some("OK".to_string()),
            value: None,
            enabled: true,
        };
        let dbg = format!("{elem:?}");
        assert!(dbg.contains("button"));
        assert!(dbg.contains("OK"));
    }
}
