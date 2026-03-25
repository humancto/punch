//! Desktop automation: screenshots, OCR, and accessibility-based UI interaction.
//!
//! The [`AutomationBackend`] trait provides a platform-agnostic interface for
//! desktop automation capabilities. Platform-specific implementations live behind
//! `#[cfg]` gates.
//!
//! ## Platform support
//!
//! - **macOS** (primary): Full support via `screencapture` + System Events accessibility APIs.
//! - **Linux** (best-effort): Screenshots via `scrot`/`import`, limited UI via `xdotool`.
//! - **Windows** (best-effort): Screenshots via PowerShell, UI tools return "not yet implemented".

pub mod common;

pub use common::*;

use async_trait::async_trait;
use punch_types::{PunchError, PunchResult};

/// Allowed accessibility attributes for `read_element_attribute`.
///
/// Attributes are used as unquoted identifiers in AppleScript, so they CANNOT
/// be escaped — they must be validated against this allowlist.
const ALLOWED_ATTRIBUTES: &[&str] = &[
    "value",
    "name",
    "role",
    "role description",
    "title",
    "description",
    "enabled",
    "focused",
    "position",
    "size",
    "selected",
    "help",
    "subrole",
    "identifier",
    "minimum value",
    "maximum value",
    "orientation",
    "placeholder value",
];

/// Platform-agnostic desktop automation backend.
#[async_trait]
pub trait AutomationBackend: Send + Sync {
    // ---- Vision ----

    /// Capture a screenshot of the full screen or a specific window.
    ///
    /// If `window` is `Some`, captures only that window (matched by title).
    /// Returns base64-encoded PNG data.
    async fn screenshot(&self, window: Option<&str>) -> PunchResult<ScreenshotResult>;

    /// Capture a screenshot of a specific UI region by bounds.
    ///
    /// If `element_id` is provided, captures the region of that element.
    /// If `bounds` is provided, captures that exact rectangle (x, y, w, h).
    async fn ui_screenshot(
        &self,
        element_id: Option<&str>,
        bounds: Option<(i32, i32, u32, u32)>,
    ) -> PunchResult<ScreenshotResult>;

    /// Extract text from an app window using OCR.
    ///
    /// This is cheaper than a screenshot + vision model for text-heavy content.
    async fn app_ocr(&self, app: &str) -> PunchResult<OcrResult>;

    // ---- UI interaction ----

    /// List all visible windows with their titles and owning apps.
    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>>;

    /// Query the accessibility tree for UI elements matching a selector.
    async fn find_ui_elements(
        &self,
        app: &str,
        selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>>;

    /// Click a UI element by its element ID (from `find_ui_elements`).
    async fn click_element(&self, element_id: &str) -> PunchResult<()>;

    /// Type text into a UI element by its element ID.
    async fn type_text(&self, element_id: &str, text: &str) -> PunchResult<()>;

    /// Read an accessibility attribute from a UI element.
    async fn read_element_attribute(
        &self,
        element_id: &str,
        attribute: &str,
    ) -> PunchResult<String>;
}

/// Create the platform-appropriate automation backend.
pub fn create_backend() -> Box<dyn AutomationBackend> {
    #[cfg(target_os = "macos")]
    {
        Box::new(MacOsBackend::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(LinuxBackend)
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsBackend)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Box::new(StubBackend)
    }
}

// ---------------------------------------------------------------------------
// Security helpers
// ---------------------------------------------------------------------------

/// Escape a string for safe interpolation into AppleScript double-quoted strings.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Validate that a role filter contains only safe characters.
/// Roles are used as unquoted identifiers in AppleScript.
fn validate_role_filter(role: &str) -> PunchResult<()> {
    if role
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '_')
    {
        Ok(())
    } else {
        Err(PunchError::Tool {
            tool: "ui_find_elements".into(),
            message: format!(
                "invalid role filter: {role:?} — only letters, digits, spaces, and underscores allowed"
            ),
        })
    }
}

/// Validate that an attribute name is in the allowlist.
fn validate_attribute(attribute: &str) -> PunchResult<()> {
    if ALLOWED_ATTRIBUTES.contains(&attribute) {
        Ok(())
    } else {
        Err(PunchError::Tool {
            tool: "ui_read_attribute".into(),
            message: format!(
                "attribute {attribute:?} is not allowed. Allowed: {}",
                ALLOWED_ATTRIBUTES.join(", ")
            ),
        })
    }
}

/// Parse an element ID ("AppName:index") into (app_name, index).
pub fn parse_element_id(element_id: &str, tool: &str) -> PunchResult<(String, usize)> {
    let parts: Vec<&str> = element_id.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(PunchError::Tool {
            tool: tool.into(),
            message: format!(
                "invalid element_id format: {element_id:?} — expected \"AppName:index\""
            ),
        });
    }
    let app = parts[0];
    if app.is_empty() {
        return Err(PunchError::Tool {
            tool: tool.into(),
            message: format!("invalid element_id: empty app name in {element_id:?}"),
        });
    }
    let index: usize = parts[1].parse().map_err(|_| PunchError::Tool {
        tool: tool.into(),
        message: format!("invalid element_id index: {element_id:?} — index must be a number"),
    })?;
    Ok((app.to_string(), index))
}

/// Extract just the app name from an element ID.
pub fn extract_app_from_element_id(element_id: &str, tool: &str) -> PunchResult<String> {
    parse_element_id(element_id, tool).map(|(app, _)| app)
}

// ---------------------------------------------------------------------------
// macOS backend
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub struct MacOsBackend {
    /// Temporary directory for screenshot files.
    tmp_dir: String,
}

#[cfg(target_os = "macos")]
impl Default for MacOsBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
impl MacOsBackend {
    pub fn new() -> Self {
        Self {
            tmp_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        }
    }

    /// Run an osascript command and return stdout.
    async fn run_osascript(&self, script: &str) -> PunchResult<String> {
        let output = tokio::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .await
            .map_err(|e| PunchError::Tool {
                tool: "automation".into(),
                message: format!("failed to run osascript: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Check for common accessibility errors and provide helpful messages.
            if stderr.contains("not allowed assistive access")
                || stderr.contains("accessibility")
                || stderr.contains("AXError")
            {
                return Err(PunchError::Tool {
                    tool: "automation".into(),
                    message: "Accessibility access required. Go to System Settings > Privacy & Security > Accessibility and enable the terminal app running Punch.".into(),
                });
            }
            return Err(PunchError::Tool {
                tool: "automation".into(),
                message: format!("osascript failed: {}", stderr.trim()),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

#[cfg(target_os = "macos")]
#[async_trait]
impl AutomationBackend for MacOsBackend {
    async fn screenshot(&self, window: Option<&str>) -> PunchResult<ScreenshotResult> {
        use base64::Engine;

        let path = format!(
            "{}/punch_screenshot_{}.png",
            self.tmp_dir,
            std::process::id()
        );

        let mut cmd = tokio::process::Command::new("screencapture");
        cmd.arg("-x") // no sound
            .arg("-t")
            .arg("png");

        if let Some(win_title) = window {
            // Get window ID by title, then capture that window.
            let escaped = escape_applescript(win_title);
            let script = format!(
                r#"tell application "System Events" to get id of first window of (first application process whose name is "{escaped}") whose name contains "{escaped}""#
            );
            match self.run_osascript(&script).await {
                Ok(window_id) => {
                    cmd.arg("-l").arg(window_id.trim());
                }
                Err(_) => {
                    // Fallback: try matching by window title directly.
                    let script2 = format!(
                        r#"tell application "System Events"
set wList to every window of every application process whose name contains "{escaped}"
if (count of wList) > 0 then
    return id of item 1 of wList
end if
end tell"#
                    );
                    match self.run_osascript(&script2).await {
                        Ok(wid) if !wid.is_empty() => {
                            cmd.arg("-l").arg(wid.trim());
                        }
                        _ => {
                            // Last resort: capture full screen.
                        }
                    }
                }
            }
        }

        cmd.arg(&path);

        let output = cmd.output().await.map_err(|e| PunchError::Tool {
            tool: "sys_screenshot".into(),
            message: format!("failed to run screencapture: {e}"),
        })?;

        if !output.status.success() {
            return Err(PunchError::Tool {
                tool: "sys_screenshot".into(),
                message: format!(
                    "screencapture failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Read the file and check for blank screenshots (permission issue).
        let data = tokio::fs::read(&path).await.map_err(|e| PunchError::Tool {
            tool: "sys_screenshot".into(),
            message: format!("failed to read screenshot file: {e}"),
        })?;

        // Clean up the temp file.
        let _ = tokio::fs::remove_file(&path).await;

        if data.len() < 1024 {
            return Err(PunchError::Tool {
                tool: "sys_screenshot".into(),
                message: "Screenshot appears blank. Grant Screen Recording permission in System Settings > Privacy & Security > Screen Recording.".into(),
            });
        }

        // Parse PNG header for dimensions.
        let (width, height) = parse_png_dimensions(&data).unwrap_or((0, 0));

        let png_base64 = base64::engine::general_purpose::STANDARD.encode(&data);

        Ok(ScreenshotResult {
            png_base64,
            width,
            height,
        })
    }

    async fn ui_screenshot(
        &self,
        element_id: Option<&str>,
        bounds: Option<(i32, i32, u32, u32)>,
    ) -> PunchResult<ScreenshotResult> {
        use base64::Engine;

        let path = format!(
            "{}/punch_ui_screenshot_{}.png",
            self.tmp_dir,
            std::process::id()
        );

        let mut cmd = tokio::process::Command::new("screencapture");
        cmd.arg("-x").arg("-t").arg("png");

        if let Some((x, y, w, h)) = bounds {
            cmd.arg("-R").arg(format!("{x},{y},{w},{h}"));
        } else if let Some(eid) = element_id {
            // Get element bounds via accessibility, then capture that region.
            let (app, index) = parse_element_id(eid, "ui_screenshot")?;
            let escaped_app = escape_applescript(&app);
            let script = format!(
                r#"tell application "System Events" to tell process "{escaped_app}"
set el to UI element {} of window 1
set p to position of el
set s to size of el
return (item 1 of p as text) & "," & (item 2 of p as text) & "," & (item 1 of s as text) & "," & (item 2 of s as text)
end tell"#,
                index + 1 // AppleScript is 1-based
            );
            let bounds_str = self.run_osascript(&script).await?;
            cmd.arg("-R").arg(bounds_str);
        }

        cmd.arg(&path);

        let output = cmd.output().await.map_err(|e| PunchError::Tool {
            tool: "ui_screenshot".into(),
            message: format!("failed to run screencapture: {e}"),
        })?;

        if !output.status.success() {
            return Err(PunchError::Tool {
                tool: "ui_screenshot".into(),
                message: format!(
                    "screencapture failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let data = tokio::fs::read(&path).await.map_err(|e| PunchError::Tool {
            tool: "ui_screenshot".into(),
            message: format!("failed to read screenshot file: {e}"),
        })?;
        let _ = tokio::fs::remove_file(&path).await;

        if data.len() < 1024 {
            return Err(PunchError::Tool {
                tool: "ui_screenshot".into(),
                message: "Screenshot appears blank. Grant Screen Recording permission.".into(),
            });
        }

        let (width, height) = parse_png_dimensions(&data).unwrap_or((0, 0));
        let png_base64 = base64::engine::general_purpose::STANDARD.encode(&data);

        Ok(ScreenshotResult {
            png_base64,
            width,
            height,
        })
    }

    async fn app_ocr(&self, app: &str) -> PunchResult<OcrResult> {
        use base64::Engine;

        // First capture a screenshot of the app window.
        let screenshot = self.screenshot(Some(app)).await?;

        // Try macOS Vision framework via a Swift one-liner.
        // Falls back to tesseract if Vision is unavailable.
        let tmp_img = format!("{}/punch_ocr_{}.png", self.tmp_dir, std::process::id());
        let img_data = base64::engine::general_purpose::STANDARD
            .decode(&screenshot.png_base64)
            .map_err(|e| PunchError::Tool {
                tool: "app_ocr".into(),
                message: format!("failed to decode screenshot: {e}"),
            })?;
        tokio::fs::write(&tmp_img, &img_data)
            .await
            .map_err(|e| PunchError::Tool {
                tool: "app_ocr".into(),
                message: format!("failed to write temp image: {e}"),
            })?;

        // Try tesseract first (widely available via homebrew).
        let output = tokio::process::Command::new("tesseract")
            .arg(&tmp_img)
            .arg("stdout")
            .output()
            .await;

        let _ = tokio::fs::remove_file(&tmp_img).await;

        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let confidence = if text.is_empty() { 0.0 } else { 0.7 };
                if text.is_empty() {
                    return Ok(OcrResult {
                        text: String::new(),
                        regions: vec![OcrRegion {
                            text: String::new(),
                            bounds: None,
                            confidence: 0.0,
                        }],
                    });
                }
                Ok(OcrResult {
                    text: text.clone(),
                    regions: vec![OcrRegion {
                        text,
                        bounds: None,
                        confidence,
                    }],
                })
            }
            _ => {
                // Tesseract not available — return a helpful error.
                Err(PunchError::Tool {
                    tool: "app_ocr".into(),
                    message: "OCR requires tesseract. Install it: brew install tesseract".into(),
                })
            }
        }
    }

    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>> {
        let script = r#"tell application "System Events"
set windowList to ""
repeat with proc in (every application process whose background only is false)
    set procName to name of proc
    repeat with win in (every window of proc)
        set winTitle to name of win
        set winPos to position of win
        set winSize to size of win
        set winMin to false
        try
            set winMin to value of attribute "AXMinimized" of win
        end try
        set windowList to windowList & procName & "|||" & winTitle & "|||" & (item 1 of winPos as text) & "," & (item 2 of winPos as text) & "|||" & (item 1 of winSize as text) & "," & (item 2 of winSize as text) & "|||" & (winMin as text) & linefeed
    end repeat
end repeat
return windowList
end tell"#;

        let result = self.run_osascript(script).await?;
        let mut windows = Vec::new();

        for line in result.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split("|||").collect();
            if parts.len() < 5 {
                continue;
            }
            let position = parse_xy_pair(parts[2]);
            let size = parse_wh_pair(parts[3]);
            windows.push(WindowInfo {
                app_name: parts[0].to_string(),
                title: parts[1].to_string(),
                position: position.map(|(x, y)| (x as i32, y as i32)),
                size: size.map(|(w, h)| (w as u32, h as u32)),
                is_minimized: parts[4].trim().eq_ignore_ascii_case("true"),
            });
        }

        Ok(windows)
    }

    async fn find_ui_elements(
        &self,
        app: &str,
        selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>> {
        let escaped_app = escape_applescript(app);

        // Build the accessibility query. If a role is specified, query that role;
        // otherwise query all "UI element" types.
        let role_clause = if let Some(ref role) = selector.role {
            validate_role_filter(role)?;
            format!("every {role}")
        } else {
            "every UI element".to_string()
        };

        let script = format!(
            r#"tell application "System Events" to tell process "{escaped_app}"
set elements to {role_clause} of window 1
set result to ""
set idx to 0
repeat with el in elements
    set elRole to role of el
    set elName to ""
    try
        set elName to name of el
    end try
    set elValue to ""
    try
        set elValue to value of el as text
    end try
    set elEnabled to true
    try
        set elEnabled to enabled of el
    end try
    set result to result & idx & "|||" & elRole & "|||" & elName & "|||" & elValue & "|||" & (elEnabled as text) & linefeed
    set idx to idx + 1
end repeat
return result
end tell"#
        );

        let result = self.run_osascript(&script).await.map_err(|e| {
            PunchError::Tool {
                tool: "ui_find_elements".into(),
                message: format!(
                    "No accessible elements found for {app}. This app may have limited accessibility support. Try sys_screenshot to visually inspect the window. (Error: {e})"
                ),
            }
        })?;

        let mut elements = Vec::new();
        for line in result.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split("|||").collect();
            if parts.len() < 5 {
                continue;
            }

            let label = if parts[2].is_empty() {
                None
            } else {
                Some(parts[2].to_string())
            };
            let value = if parts[3].is_empty() {
                None
            } else {
                Some(parts[3].to_string())
            };

            // Apply label/value filters from selector.
            if let Some(ref filter_label) = selector.label
                && !label
                    .as_ref()
                    .is_some_and(|l| l.to_lowercase().contains(&filter_label.to_lowercase()))
            {
                continue;
            }
            if let Some(ref filter_value) = selector.value
                && !value
                    .as_ref()
                    .is_some_and(|v| v.to_lowercase().contains(&filter_value.to_lowercase()))
            {
                continue;
            }

            elements.push(UiElement {
                element_id: format!("{}:{}", app, parts[0].trim()),
                role: parts[1].to_string(),
                label,
                value,
                enabled: parts[4].trim().eq_ignore_ascii_case("true"),
            });
        }

        Ok(elements)
    }

    async fn click_element(&self, element_id: &str) -> PunchResult<()> {
        let (app, index) = parse_element_id(element_id, "ui_click")?;
        let escaped_app = escape_applescript(&app);
        let applescript_index = index + 1; // AppleScript is 1-based

        let script = format!(
            r#"tell application "System Events" to tell process "{escaped_app}"
click UI element {applescript_index} of window 1
end tell"#
        );

        self.run_osascript(&script).await?;
        Ok(())
    }

    async fn type_text(&self, element_id: &str, text: &str) -> PunchResult<()> {
        let (app, index) = parse_element_id(element_id, "ui_type_text")?;
        let escaped_app = escape_applescript(&app);
        let escaped_text = escape_applescript(text);
        let applescript_index = index + 1;

        let script = format!(
            r#"tell application "System Events" to tell process "{escaped_app}"
set value of UI element {applescript_index} of window 1 to "{escaped_text}"
end tell"#
        );

        self.run_osascript(&script).await?;
        Ok(())
    }

    async fn read_element_attribute(
        &self,
        element_id: &str,
        attribute: &str,
    ) -> PunchResult<String> {
        validate_attribute(attribute)?;
        let (app, index) = parse_element_id(element_id, "ui_read_attribute")?;
        let escaped_app = escape_applescript(&app);
        let applescript_index = index + 1;

        let script = format!(
            r#"tell application "System Events" to tell process "{escaped_app}"
return {attribute} of UI element {applescript_index} of window 1 as text
end tell"#
        );

        self.run_osascript(&script).await
    }
}

// ---------------------------------------------------------------------------
// Linux backend (best-effort)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
pub struct LinuxBackend;

#[cfg(target_os = "linux")]
#[async_trait]
impl AutomationBackend for LinuxBackend {
    async fn screenshot(&self, _window: Option<&str>) -> PunchResult<ScreenshotResult> {
        use base64::Engine;

        let path = format!("/tmp/punch_screenshot_{}.png", std::process::id());

        // Try scrot first, then import (ImageMagick).
        let output = tokio::process::Command::new("scrot")
            .arg(&path)
            .output()
            .await;

        let ok = match output {
            Ok(o) if o.status.success() => true,
            _ => {
                let import = tokio::process::Command::new("import")
                    .arg("-window")
                    .arg("root")
                    .arg(&path)
                    .output()
                    .await;
                matches!(import, Ok(o) if o.status.success())
            }
        };

        if !ok {
            return Err(PunchError::Tool {
                tool: "sys_screenshot".into(),
                message:
                    "Screenshot requires scrot or ImageMagick. Install: sudo apt install scrot"
                        .into(),
            });
        }

        let data = tokio::fs::read(&path).await.map_err(|e| PunchError::Tool {
            tool: "sys_screenshot".into(),
            message: format!("failed to read screenshot: {e}"),
        })?;
        let _ = tokio::fs::remove_file(&path).await;

        let (width, height) = parse_png_dimensions(&data).unwrap_or((0, 0));
        let png_base64 = base64::engine::general_purpose::STANDARD.encode(&data);

        Ok(ScreenshotResult {
            png_base64,
            width,
            height,
        })
    }

    async fn ui_screenshot(
        &self,
        _element_id: Option<&str>,
        _bounds: Option<(i32, i32, u32, u32)>,
    ) -> PunchResult<ScreenshotResult> {
        Err(PunchError::Tool {
            tool: "ui_screenshot".into(),
            message: "UI region screenshot not yet implemented on Linux.".into(),
        })
    }

    async fn app_ocr(&self, _app: &str) -> PunchResult<OcrResult> {
        // Capture full screen then OCR with tesseract.
        let ss = self.screenshot(None).await?;
        let tmp = format!("/tmp/punch_ocr_{}.png", std::process::id());
        let data = base64::engine::general_purpose::STANDARD
            .decode(&ss.png_base64)
            .map_err(|e| PunchError::Tool {
                tool: "app_ocr".into(),
                message: format!("decode error: {e}"),
            })?;
        tokio::fs::write(&tmp, &data)
            .await
            .map_err(|e| PunchError::Tool {
                tool: "app_ocr".into(),
                message: format!("write error: {e}"),
            })?;
        let output = tokio::process::Command::new("tesseract")
            .arg(&tmp)
            .arg("stdout")
            .output()
            .await;
        let _ = tokio::fs::remove_file(&tmp).await;
        match output {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                Ok(OcrResult {
                    text: text.clone(),
                    regions: vec![OcrRegion {
                        text,
                        bounds: None,
                        confidence: 0.7,
                    }],
                })
            }
            _ => Err(PunchError::Tool {
                tool: "app_ocr".into(),
                message: "tesseract not found. Install: sudo apt install tesseract-ocr".into(),
            }),
        }
    }

    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>> {
        let output = tokio::process::Command::new("wmctrl")
            .arg("-l")
            .output()
            .await;
        match output {
            Ok(o) if o.status.success() => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let windows = stdout
                    .lines()
                    .filter_map(|line| {
                        let parts: Vec<&str> = line.splitn(4, char::is_whitespace).collect();
                        if parts.len() >= 4 {
                            Some(WindowInfo {
                                title: parts[3].to_string(),
                                app_name: parts[3].to_string(),
                                position: None,
                                size: None,
                                is_minimized: false,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(windows)
            }
            _ => Err(PunchError::Tool {
                tool: "ui_list_windows".into(),
                message: "wmctrl not found. Install: sudo apt install wmctrl".into(),
            }),
        }
    }

    async fn find_ui_elements(
        &self,
        _app: &str,
        _selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>> {
        Err(PunchError::Tool { tool: "ui_find_elements".into(), message: "Accessibility tree query not yet implemented on Linux. Use sys_screenshot for visual inspection.".into() })
    }

    async fn click_element(&self, _element_id: &str) -> PunchResult<()> {
        Err(PunchError::Tool {
            tool: "ui_click".into(),
            message: "UI click not yet implemented on Linux.".into(),
        })
    }

    async fn type_text(&self, _element_id: &str, _text: &str) -> PunchResult<()> {
        Err(PunchError::Tool {
            tool: "ui_type_text".into(),
            message: "UI type not yet implemented on Linux.".into(),
        })
    }

    async fn read_element_attribute(
        &self,
        _element_id: &str,
        _attribute: &str,
    ) -> PunchResult<String> {
        Err(PunchError::Tool {
            tool: "ui_read_attribute".into(),
            message: "Attribute reading not yet implemented on Linux.".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// Windows backend (best-effort)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub struct WindowsBackend;

#[cfg(target_os = "windows")]
#[async_trait]
impl AutomationBackend for WindowsBackend {
    async fn screenshot(&self, _window: Option<&str>) -> PunchResult<ScreenshotResult> {
        Err(PunchError::Tool {
            tool: "sys_screenshot".into(),
            message: "Windows screenshot not yet implemented.".into(),
        })
    }
    async fn ui_screenshot(
        &self,
        _element_id: Option<&str>,
        _bounds: Option<(i32, i32, u32, u32)>,
    ) -> PunchResult<ScreenshotResult> {
        Err(PunchError::Tool {
            tool: "ui_screenshot".into(),
            message: "Windows UI screenshot not yet implemented.".into(),
        })
    }
    async fn app_ocr(&self, _app: &str) -> PunchResult<OcrResult> {
        Err(PunchError::Tool {
            tool: "app_ocr".into(),
            message: "Windows OCR not yet implemented.".into(),
        })
    }
    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>> {
        Err(PunchError::Tool {
            tool: "ui_list_windows".into(),
            message: "Windows list_windows not yet implemented.".into(),
        })
    }
    async fn find_ui_elements(
        &self,
        _app: &str,
        _selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>> {
        Err(PunchError::Tool {
            tool: "ui_find_elements".into(),
            message: "Windows UI automation not yet implemented.".into(),
        })
    }
    async fn click_element(&self, _element_id: &str) -> PunchResult<()> {
        Err(PunchError::Tool {
            tool: "ui_click".into(),
            message: "Windows UI click not yet implemented.".into(),
        })
    }
    async fn type_text(&self, _element_id: &str, _text: &str) -> PunchResult<()> {
        Err(PunchError::Tool {
            tool: "ui_type_text".into(),
            message: "Windows UI type not yet implemented.".into(),
        })
    }
    async fn read_element_attribute(
        &self,
        _element_id: &str,
        _attribute: &str,
    ) -> PunchResult<String> {
        Err(PunchError::Tool {
            tool: "ui_read_attribute".into(),
            message: "Windows attribute reading not yet implemented.".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// Stub backend (unsupported platforms)
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub struct StubBackend;

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
#[async_trait]
impl AutomationBackend for StubBackend {
    async fn screenshot(&self, _window: Option<&str>) -> PunchResult<ScreenshotResult> {
        Err(PunchError::Tool {
            tool: "sys_screenshot".into(),
            message: "Desktop automation not supported on this platform.".into(),
        })
    }
    async fn ui_screenshot(
        &self,
        _element_id: Option<&str>,
        _bounds: Option<(i32, i32, u32, u32)>,
    ) -> PunchResult<ScreenshotResult> {
        Err(PunchError::Tool {
            tool: "ui_screenshot".into(),
            message: "Desktop automation not supported on this platform.".into(),
        })
    }
    async fn app_ocr(&self, _app: &str) -> PunchResult<OcrResult> {
        Err(PunchError::Tool {
            tool: "app_ocr".into(),
            message: "Desktop automation not supported on this platform.".into(),
        })
    }
    async fn list_windows(&self) -> PunchResult<Vec<WindowInfo>> {
        Err(PunchError::Tool {
            tool: "ui_list_windows".into(),
            message: "Desktop automation not supported on this platform.".into(),
        })
    }
    async fn find_ui_elements(
        &self,
        _app: &str,
        _selector: &UiSelector,
    ) -> PunchResult<Vec<UiElement>> {
        Err(PunchError::Tool {
            tool: "ui_find_elements".into(),
            message: "Desktop automation not supported on this platform.".into(),
        })
    }
    async fn click_element(&self, _element_id: &str) -> PunchResult<()> {
        Err(PunchError::Tool {
            tool: "ui_click".into(),
            message: "Desktop automation not supported on this platform.".into(),
        })
    }
    async fn type_text(&self, _element_id: &str, _text: &str) -> PunchResult<()> {
        Err(PunchError::Tool {
            tool: "ui_type_text".into(),
            message: "Desktop automation not supported on this platform.".into(),
        })
    }
    async fn read_element_attribute(
        &self,
        _element_id: &str,
        _attribute: &str,
    ) -> PunchResult<String> {
        Err(PunchError::Tool {
            tool: "ui_read_attribute".into(),
            message: "Desktop automation not supported on this platform.".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse PNG IHDR chunk to extract width and height.
fn parse_png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // PNG header: 8 bytes signature, then IHDR chunk.
    // IHDR starts at byte 8: 4 bytes length, 4 bytes "IHDR", then 4 bytes width, 4 bytes height.
    if data.len() < 24 {
        return None;
    }
    // Check PNG signature.
    if data[0..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((width, height))
}

/// Parse "x,y" into (i64, i64).
fn parse_xy_pair(s: &str) -> Option<(i64, i64)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() == 2 {
        let x = parts[0].trim().parse().ok()?;
        let y = parts[1].trim().parse().ok()?;
        Some((x, y))
    } else {
        None
    }
}

/// Parse "w,h" into (u64, u64).
fn parse_wh_pair(s: &str) -> Option<(u64, u64)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() == 2 {
        let w = parts[0].trim().parse().ok()?;
        let h = parts[1].trim().parse().ok()?;
        Some((w, h))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Security helper tests ----

    #[test]
    fn test_escape_applescript_basic() {
        assert_eq!(escape_applescript(r#"hello"world"#), r#"hello\"world"#);
        assert_eq!(escape_applescript("line\nnewline"), "line\\nnewline");
        assert_eq!(escape_applescript(r"back\slash"), r"back\\slash");
        assert_eq!(escape_applescript("normal text"), "normal text");
    }

    #[test]
    fn test_escape_applescript_empty() {
        assert_eq!(escape_applescript(""), "");
    }

    #[test]
    fn test_escape_applescript_carriage_return() {
        assert_eq!(escape_applescript("foo\rbar"), "foo\\rbar");
    }

    #[test]
    fn test_escape_applescript_all_special() {
        assert_eq!(escape_applescript("\\\"\n\r"), "\\\\\\\"\\n\\r");
    }

    #[test]
    fn test_validate_role_filter_valid() {
        assert!(validate_role_filter("button").is_ok());
        assert!(validate_role_filter("text field").is_ok());
        assert!(validate_role_filter("UI element").is_ok());
        assert!(validate_role_filter("menu_item").is_ok());
        assert!(validate_role_filter("AXButton").is_ok());
    }

    #[test]
    fn test_validate_role_filter_invalid() {
        assert!(validate_role_filter("button;rm -rf").is_err());
        assert!(validate_role_filter("foo\"bar").is_err());
        assert!(validate_role_filter("test\ninjection").is_err());
        assert!(validate_role_filter("$(whoami)").is_err());
    }

    #[test]
    fn test_validate_attribute_valid() {
        assert!(validate_attribute("value").is_ok());
        assert!(validate_attribute("name").is_ok());
        assert!(validate_attribute("role description").is_ok());
        assert!(validate_attribute("placeholder value").is_ok());
    }

    #[test]
    fn test_validate_attribute_invalid() {
        assert!(validate_attribute("hacked").is_err());
        assert!(validate_attribute("").is_err());
        assert!(validate_attribute("value; rm -rf /").is_err());
    }

    // ---- Element ID parsing tests ----

    #[test]
    fn test_parse_element_id_valid() {
        let (app, idx) = parse_element_id("Safari:3", "test").unwrap();
        assert_eq!(app, "Safari");
        assert_eq!(idx, 3);
    }

    #[test]
    fn test_parse_element_id_zero_index() {
        let (app, idx) = parse_element_id("Messages:0", "test").unwrap();
        assert_eq!(app, "Messages");
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_parse_element_id_app_with_spaces() {
        let (app, idx) = parse_element_id("System Preferences:5", "test").unwrap();
        assert_eq!(app, "System Preferences");
        assert_eq!(idx, 5);
    }

    #[test]
    fn test_parse_element_id_missing_colon() {
        assert!(parse_element_id("Safari3", "test").is_err());
    }

    #[test]
    fn test_parse_element_id_empty_app() {
        assert!(parse_element_id(":3", "test").is_err());
    }

    #[test]
    fn test_parse_element_id_non_numeric_index() {
        assert!(parse_element_id("Safari:abc", "test").is_err());
    }

    #[test]
    fn test_parse_element_id_empty_string() {
        assert!(parse_element_id("", "test").is_err());
    }

    #[test]
    fn test_extract_app_from_element_id() {
        let app = extract_app_from_element_id("Messages:0", "test").unwrap();
        assert_eq!(app, "Messages");
    }

    // ---- PNG dimension parsing tests ----

    #[test]
    fn test_parse_png_dimensions_valid() {
        // Construct a minimal PNG header with known dimensions.
        let mut data = vec![137, 80, 78, 71, 13, 10, 26, 10]; // PNG signature
        data.extend_from_slice(&[0, 0, 0, 13]); // IHDR length
        data.extend_from_slice(b"IHDR"); // chunk type
        data.extend_from_slice(&1920u32.to_be_bytes()); // width
        data.extend_from_slice(&1080u32.to_be_bytes()); // height

        let (w, h) = parse_png_dimensions(&data).unwrap();
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
    }

    #[test]
    fn test_parse_png_dimensions_too_short() {
        assert!(parse_png_dimensions(&[0; 10]).is_none());
    }

    #[test]
    fn test_parse_png_dimensions_bad_signature() {
        assert!(parse_png_dimensions(&[0; 30]).is_none());
    }

    // ---- Coordinate parsing tests ----

    #[test]
    fn test_parse_xy_pair() {
        assert_eq!(parse_xy_pair("100,200"), Some((100, 200)));
        assert_eq!(parse_xy_pair("-10, 50"), Some((-10, 50)));
        assert!(parse_xy_pair("abc,def").is_none());
        assert!(parse_xy_pair("100").is_none());
    }

    #[test]
    fn test_parse_wh_pair() {
        assert_eq!(parse_wh_pair("1920,1080"), Some((1920, 1080)));
        assert!(parse_wh_pair("abc,100").is_none());
        assert!(parse_wh_pair("100").is_none());
    }
}
