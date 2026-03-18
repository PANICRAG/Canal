//! Platform-native automation tools
//!
//! Provides native Rust implementations for macOS and Windows automation,
//! eliminating the need for external Node.js/Python MCP servers.
//!
//! ## macOS Tools (namespace: mac)
//! - `osascript` - Execute AppleScript commands
//! - `screenshot` - Capture screen
//! - `app_control` - Control applications
//!
//! ## Windows Tools (namespace: win)
//! - `click` - Click at coordinates
//! - `type_text` - Type text
//! - `scroll` - Scroll vertically/horizontally
//! - `move_mouse` - Move/drag mouse
//! - `shortcut` - Keyboard shortcuts
//! - `wait` - Pause execution
//! - `snapshot` - Screenshot + accessibility tree
//! - `app` - Launch/resize/switch apps
//! - `shell` - Execute PowerShell commands

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

use super::protocol::ToolCallResult;
use crate::error::{Error, Result};

/// Get the default screenshot path using the app's cache directory
/// Falls back to /tmp if cache directory is not available
fn default_screenshot_path() -> &'static str {
    // Use a lazily-initialized static string for the path
    use std::sync::OnceLock;
    static PATH: OnceLock<String> = OnceLock::new();

    PATH.get_or_init(|| {
        if let Some(cache_dir) = dirs::cache_dir() {
            let screenshot_dir = cache_dir.join("canal").join("screenshots");
            // Create directory if it doesn't exist
            let _ = std::fs::create_dir_all(&screenshot_dir);
            screenshot_dir
                .join("screenshot.png")
                .to_string_lossy()
                .to_string()
        } else if let Some(home) = dirs::home_dir() {
            let screenshot_dir = home.join(".canal").join("screenshots");
            let _ = std::fs::create_dir_all(&screenshot_dir);
            screenshot_dir
                .join("screenshot.png")
                .to_string_lossy()
                .to_string()
        } else {
            "/tmp/screenshot.png".to_string()
        }
    })
    .as_str()
}

/// Platform automation executor
pub struct PlatformAutomation {
    /// Default timeout for commands
    #[allow(dead_code)]
    default_timeout: Duration,
}

impl Default for PlatformAutomation {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformAutomation {
    pub fn new() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
        }
    }

    #[allow(dead_code)]
    pub fn with_timeout(mut self, timeout_duration: Duration) -> Self {
        self.default_timeout = timeout_duration;
        self
    }

    /// Check if this platform is supported
    pub fn is_supported(&self) -> bool {
        cfg!(target_os = "macos") || cfg!(target_os = "windows")
    }

    /// Get the current platform namespace
    pub fn namespace(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "mac"
        } else if cfg!(target_os = "windows") {
            "win"
        } else {
            "unsupported"
        }
    }

    /// Execute a platform automation tool
    pub async fn execute(&self, tool_name: &str, arguments: Value) -> Result<ToolCallResult> {
        #[cfg(target_os = "macos")]
        {
            return self.execute_macos(tool_name, arguments).await;
        }

        #[cfg(target_os = "windows")]
        {
            return self.execute_windows(tool_name, arguments).await;
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (tool_name, arguments);
            Ok(ToolCallResult::error(
                "Platform automation not supported on this OS",
            ))
        }
    }

    // ==================== macOS Implementation ====================

    #[cfg(target_os = "macos")]
    async fn execute_macos(&self, tool_name: &str, arguments: Value) -> Result<ToolCallResult> {
        match tool_name {
            "osascript" => self.macos_osascript(arguments).await,
            "screenshot" => self.macos_screenshot(arguments).await,
            "app_control" => self.macos_app_control(arguments).await,
            "open_url" => self.macos_open_url(arguments).await,
            "notify" => self.macos_notify(arguments).await,
            "clipboard_read" => self.macos_clipboard_read().await,
            "clipboard_write" => self.macos_clipboard_write(arguments).await,
            "get_frontmost_app" => self.macos_get_frontmost_app().await,
            "list_running_apps" => self.macos_list_running_apps().await,
            _ => Ok(ToolCallResult::error(format!(
                "Unknown macOS tool: {}",
                tool_name
            ))),
        }
    }

    /// Execute AppleScript via osascript
    #[cfg(target_os = "macos")]
    async fn macos_osascript(&self, arguments: Value) -> Result<ToolCallResult> {
        let script = arguments
            .get("script")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'script' parameter".to_string()))?;

        let timeout_secs = arguments
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        let result = timeout(Duration::from_secs(timeout_secs), async {
            let output = Command::new("osascript")
                .arg("-e")
                .arg(script)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?
                .wait_with_output()
                .await?;

            Ok::<_, std::io::Error>(output)
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    Ok(ToolCallResult::text(
                        json!({
                            "success": true,
                            "output": stdout
                        })
                        .to_string(),
                    ))
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    Ok(ToolCallResult::error(format!(
                        "AppleScript error: {}",
                        stderr
                    )))
                }
            }
            Ok(Err(e)) => Ok(ToolCallResult::error(format!("Failed to execute: {}", e))),
            Err(_) => Ok(ToolCallResult::error("Command timed out")),
        }
    }

    /// Capture screenshot on macOS
    #[cfg(target_os = "macos")]
    async fn macos_screenshot(&self, arguments: Value) -> Result<ToolCallResult> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| default_screenshot_path());

        let region = arguments.get("region");

        let mut cmd = Command::new("screencapture");
        cmd.arg("-x"); // No sound

        if let Some(region) = region {
            if let (Some(x), Some(y), Some(w), Some(h)) = (
                region.get("x").and_then(|v| v.as_i64()),
                region.get("y").and_then(|v| v.as_i64()),
                region.get("width").and_then(|v| v.as_i64()),
                region.get("height").and_then(|v| v.as_i64()),
            ) {
                cmd.arg("-R").arg(format!("{},{},{},{}", x, y, w, h));
            }
        }

        cmd.arg(path);

        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;

        if output.status.success() {
            Ok(ToolCallResult::text(
                json!({
                    "success": true,
                    "path": path,
                    "message": "Screenshot captured"
                })
                .to_string(),
            ))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Ok(ToolCallResult::error(format!(
                "Screenshot failed: {}",
                stderr
            )))
        }
    }

    /// Sanitize a string for safe interpolation into AppleScript quoted contexts.
    /// Rejects strings containing characters that could break out of quotes.
    #[cfg(target_os = "macos")]
    fn sanitize_applescript_string(input: &str) -> Result<String> {
        // Reject characters that can break out of AppleScript string context
        if input.contains('"')
            || input.contains('\\')
            || input.contains('\n')
            || input.contains('\r')
            || input.contains('\0')
        {
            return Err(Error::InvalidInput(format!(
                "Input contains unsafe characters for AppleScript: {}",
                input
            )));
        }
        // Only allow printable ASCII + common Unicode letters for app names
        if input.len() > 256 {
            return Err(Error::InvalidInput(
                "Input too long for AppleScript interpolation".to_string(),
            ));
        }
        Ok(input.to_string())
    }

    /// Sanitize a string for safe interpolation into PowerShell single-quoted contexts.
    /// Escapes single quotes and rejects characters that could enable injection.
    #[cfg(target_os = "windows")]
    fn sanitize_powershell_string(input: &str) -> Result<String> {
        // Reject backticks (PowerShell escape character), $variables, and subshell syntax
        if input.contains('`') || input.contains('$') || input.contains('\0') {
            return Err(Error::InvalidInput(format!(
                "Input contains unsafe characters for PowerShell: backtick, $, or null"
            )));
        }
        // Length limit to prevent abuse
        if input.len() > 4096 {
            return Err(Error::InvalidInput(
                "Input too long for PowerShell interpolation".to_string(),
            ));
        }
        // Escape single quotes by doubling them (PowerShell convention)
        Ok(input.replace('\'', "''"))
    }

    /// Control applications on macOS
    #[cfg(target_os = "macos")]
    async fn macos_app_control(&self, arguments: Value) -> Result<ToolCallResult> {
        let action = arguments
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'action' parameter".to_string()))?;

        let app_name = arguments.get("app").and_then(|v| v.as_str());

        let script = match action {
            "launch" => {
                let app = app_name.ok_or_else(|| {
                    Error::InvalidInput("Missing 'app' parameter for launch".to_string())
                })?;
                let safe_app = Self::sanitize_applescript_string(app)?;
                format!("tell application \"{}\" to activate", safe_app)
            }
            "quit" => {
                let app = app_name.ok_or_else(|| {
                    Error::InvalidInput("Missing 'app' parameter for quit".to_string())
                })?;
                let safe_app = Self::sanitize_applescript_string(app)?;
                format!("tell application \"{}\" to quit", safe_app)
            }
            "hide" => {
                let app = app_name.ok_or_else(|| {
                    Error::InvalidInput("Missing 'app' parameter for hide".to_string())
                })?;
                let safe_app = Self::sanitize_applescript_string(app)?;
                format!(
                    "tell application \"System Events\" to set visible of process \"{}\" to false",
                    safe_app
                )
            }
            "show" => {
                let app = app_name.ok_or_else(|| {
                    Error::InvalidInput("Missing 'app' parameter for show".to_string())
                })?;
                let safe_app = Self::sanitize_applescript_string(app)?;
                format!(
                    "tell application \"System Events\" to set visible of process \"{}\" to true",
                    safe_app
                )
            }
            "minimize" => {
                let app = app_name.ok_or_else(|| {
                    Error::InvalidInput("Missing 'app' parameter for minimize".to_string())
                })?;
                let safe_app = Self::sanitize_applescript_string(app)?;
                format!(
                    "tell application \"System Events\" to set miniaturized of windows of process \"{}\" to true",
                    safe_app
                )
            }
            _ => {
                return Ok(ToolCallResult::error(format!("Unknown action: {}", action)));
            }
        };

        self.macos_osascript(json!({"script": script})).await
    }

    /// Open URL on macOS
    #[cfg(target_os = "macos")]
    async fn macos_open_url(&self, arguments: Value) -> Result<ToolCallResult> {
        let url = arguments
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'url' parameter".to_string()))?;

        // R3-C7: Validate URL scheme — only allow safe web protocols
        let url_lower = url.to_lowercase();
        let allowed_schemes = ["http://", "https://", "mailto:"];
        if !allowed_schemes
            .iter()
            .any(|scheme| url_lower.starts_with(scheme))
        {
            return Err(Error::InvalidInput(format!(
                "URL scheme not allowed. Only http://, https://, and mailto: are permitted. Got: {}",
                url.chars().take(50).collect::<String>()
            )));
        }

        let output = Command::new("open")
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;

        if output.status.success() {
            Ok(ToolCallResult::text(
                json!({
                    "success": true,
                    "url": url
                })
                .to_string(),
            ))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Ok(ToolCallResult::error(format!(
                "Failed to open URL: {}",
                stderr
            )))
        }
    }

    /// Show notification on macOS
    #[cfg(target_os = "macos")]
    async fn macos_notify(&self, arguments: Value) -> Result<ToolCallResult> {
        let title = arguments
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Notification");
        let message = arguments
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'message' parameter".to_string()))?;

        let safe_message = Self::sanitize_applescript_string(message)?;
        let safe_title = Self::sanitize_applescript_string(title)?;
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            safe_message, safe_title
        );

        self.macos_osascript(json!({"script": script})).await
    }

    /// Read clipboard on macOS
    #[cfg(target_os = "macos")]
    async fn macos_clipboard_read(&self) -> Result<ToolCallResult> {
        let output = Command::new("pbpaste")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;

        if output.status.success() {
            let content = String::from_utf8_lossy(&output.stdout).to_string();
            Ok(ToolCallResult::text(
                json!({
                    "success": true,
                    "content": content
                })
                .to_string(),
            ))
        } else {
            Ok(ToolCallResult::error("Failed to read clipboard"))
        }
    }

    /// Write to clipboard on macOS
    #[cfg(target_os = "macos")]
    async fn macos_clipboard_write(&self, arguments: Value) -> Result<ToolCallResult> {
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'content' parameter".to_string()))?;

        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(stdin) = child.stdin.as_mut() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(content.as_bytes()).await?;
        }

        let output = child.wait_with_output().await?;

        if output.status.success() {
            Ok(ToolCallResult::text(
                json!({
                    "success": true,
                    "message": "Content copied to clipboard"
                })
                .to_string(),
            ))
        } else {
            Ok(ToolCallResult::error("Failed to write to clipboard"))
        }
    }

    /// Get frontmost app on macOS
    #[cfg(target_os = "macos")]
    async fn macos_get_frontmost_app(&self) -> Result<ToolCallResult> {
        let script = r#"
            tell application "System Events"
                set frontApp to first application process whose frontmost is true
                return name of frontApp
            end tell
        "#;

        self.macos_osascript(json!({"script": script})).await
    }

    /// List running apps on macOS
    #[cfg(target_os = "macos")]
    async fn macos_list_running_apps(&self) -> Result<ToolCallResult> {
        let script = r#"
            tell application "System Events"
                set appList to name of every application process whose background only is false
                return appList
            end tell
        "#;

        self.macos_osascript(json!({"script": script})).await
    }

    // ==================== Windows Implementation ====================
    // Note: Windows implementation requires the `windows` crate.
    // For now, we provide shell-based implementations using PowerShell.

    #[cfg(target_os = "windows")]
    async fn execute_windows(&self, tool_name: &str, arguments: Value) -> Result<ToolCallResult> {
        match tool_name {
            "click" => self.windows_click(arguments).await,
            "type_text" => self.windows_type_text(arguments).await,
            "scroll" => self.windows_scroll(arguments).await,
            "move_mouse" => self.windows_move_mouse(arguments).await,
            "shortcut" => self.windows_shortcut(arguments).await,
            "wait" => self.windows_wait(arguments).await,
            "snapshot" => self.windows_snapshot(arguments).await,
            "app" => self.windows_app(arguments).await,
            "shell" => self.windows_shell(arguments).await,
            "clipboard_read" => self.windows_clipboard_read().await,
            "clipboard_write" => self.windows_clipboard_write(arguments).await,
            _ => Ok(ToolCallResult::error(format!(
                "Unknown Windows tool: {}",
                tool_name
            ))),
        }
    }

    /// Click at coordinates on Windows (using PowerShell)
    #[cfg(target_os = "windows")]
    async fn windows_click(&self, arguments: Value) -> Result<ToolCallResult> {
        let x = arguments
            .get("x")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::InvalidInput("Missing 'x' coordinate".to_string()))?;

        let y = arguments
            .get("y")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::InvalidInput("Missing 'y' coordinate".to_string()))?;

        let button = arguments
            .get("button")
            .and_then(|v| v.as_str())
            .unwrap_or("left");

        let click_type = arguments
            .get("click_type")
            .and_then(|v| v.as_str())
            .unwrap_or("single");

        // PowerShell script to click
        let script = format!(
            r#"
            Add-Type -AssemblyName System.Windows.Forms
            [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point({}, {})
            $signature = @'
            [DllImport("user32.dll", CharSet = CharSet.Auto, CallingConvention = CallingConvention.StdCall)]
            public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint cButtons, uint dwExtraInfo);
            '@
            $SendMouseClick = Add-Type -MemberDefinition $signature -Name "Win32MouseEvent" -Namespace Win32Functions -PassThru
            {}
            "#,
            x, y,
            match (button, click_type) {
                ("left", "double") => "$SendMouseClick::mouse_event(0x0002, 0, 0, 0, 0); $SendMouseClick::mouse_event(0x0004, 0, 0, 0, 0); Start-Sleep -Milliseconds 100; $SendMouseClick::mouse_event(0x0002, 0, 0, 0, 0); $SendMouseClick::mouse_event(0x0004, 0, 0, 0, 0)",
                ("right", _) => "$SendMouseClick::mouse_event(0x0008, 0, 0, 0, 0); $SendMouseClick::mouse_event(0x0010, 0, 0, 0, 0)",
                _ => "$SendMouseClick::mouse_event(0x0002, 0, 0, 0, 0); $SendMouseClick::mouse_event(0x0004, 0, 0, 0, 0)",
            }
        );

        self.run_powershell(&script).await
    }

    /// Type text on Windows
    #[cfg(target_os = "windows")]
    async fn windows_type_text(&self, arguments: Value) -> Result<ToolCallResult> {
        let text = arguments
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'text' parameter".to_string()))?;

        let clear = arguments
            .get("clear")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let script = if clear {
            format!(
                r#"
                Add-Type -AssemblyName System.Windows.Forms
                [System.Windows.Forms.SendKeys]::SendWait("^a")
                Start-Sleep -Milliseconds 50
                [System.Windows.Forms.SendKeys]::SendWait("{}")
                "#,
                escape_sendkeys(text)
            )
        } else {
            format!(
                r#"
                Add-Type -AssemblyName System.Windows.Forms
                [System.Windows.Forms.SendKeys]::SendWait("{}")
                "#,
                escape_sendkeys(text)
            )
        };

        self.run_powershell(&script).await
    }

    /// Scroll on Windows
    #[cfg(target_os = "windows")]
    async fn windows_scroll(&self, arguments: Value) -> Result<ToolCallResult> {
        let direction = arguments
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("down");

        let amount = arguments
            .get("amount")
            .and_then(|v| v.as_i64())
            .unwrap_or(3);

        let wheel_delta = match direction {
            "up" => amount * 120,
            _ => -amount * 120,
        };

        let script = format!(
            r#"
            $signature = @'
            [DllImport("user32.dll", CharSet = CharSet.Auto, CallingConvention = CallingConvention.StdCall)]
            public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint cButtons, uint dwExtraInfo);
            '@
            $SendMouseClick = Add-Type -MemberDefinition $signature -Name "Win32MouseEvent" -Namespace Win32Functions -PassThru
            $SendMouseClick::mouse_event(0x0800, 0, 0, {}, 0)
            "#,
            wheel_delta
        );

        self.run_powershell(&script).await
    }

    /// Move mouse on Windows
    #[cfg(target_os = "windows")]
    async fn windows_move_mouse(&self, arguments: Value) -> Result<ToolCallResult> {
        let x = arguments
            .get("x")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::InvalidInput("Missing 'x' coordinate".to_string()))?;

        let y = arguments
            .get("y")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::InvalidInput("Missing 'y' coordinate".to_string()))?;

        let script = format!(
            r#"
            Add-Type -AssemblyName System.Windows.Forms
            [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point({}, {})
            "#,
            x, y
        );

        self.run_powershell(&script).await
    }

    /// Execute keyboard shortcut on Windows
    #[cfg(target_os = "windows")]
    async fn windows_shortcut(&self, arguments: Value) -> Result<ToolCallResult> {
        let keys = arguments
            .get("keys")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'keys' parameter".to_string()))?;

        // Convert shortcut format to SendKeys format
        let sendkeys = convert_shortcut_to_sendkeys(keys);

        let script = format!(
            r#"
            Add-Type -AssemblyName System.Windows.Forms
            [System.Windows.Forms.SendKeys]::SendWait("{}")
            "#,
            sendkeys
        );

        self.run_powershell(&script).await
    }

    /// Wait/pause on Windows
    #[cfg(target_os = "windows")]
    async fn windows_wait(&self, arguments: Value) -> Result<ToolCallResult> {
        let duration_ms = arguments
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000);

        tokio::time::sleep(Duration::from_millis(duration_ms)).await;

        Ok(ToolCallResult::text(
            json!({
                "success": true,
                "waited_ms": duration_ms
            })
            .to_string(),
        ))
    }

    /// Take snapshot on Windows
    #[cfg(target_os = "windows")]
    async fn windows_snapshot(&self, arguments: Value) -> Result<ToolCallResult> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("C:\\temp\\screenshot.png");

        let safe_path = Self::sanitize_powershell_string(path)?;
        let script = format!(
            r#"
            Add-Type -AssemblyName System.Windows.Forms
            $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
            $bitmap = New-Object System.Drawing.Bitmap($screen.Width, $screen.Height)
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
            $graphics.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
            $bitmap.Save('{}')
            $graphics.Dispose()
            $bitmap.Dispose()
            Write-Output 'Screenshot saved'
            "#,
            safe_path
        );

        self.run_powershell(&script).await
    }

    /// Control applications on Windows
    #[cfg(target_os = "windows")]
    async fn windows_app(&self, arguments: Value) -> Result<ToolCallResult> {
        let action = arguments
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'action' parameter".to_string()))?;

        match action {
            "launch" => {
                let app = arguments
                    .get("app")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::InvalidInput("Missing 'app' parameter".to_string()))?;

                let safe_app = Self::sanitize_powershell_string(app)?;
                let script = format!("Start-Process '{}'", safe_app);
                self.run_powershell(&script).await
            }
            "focus" => {
                let title = arguments
                    .get("title")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::InvalidInput("Missing 'title' parameter".to_string()))?;

                let script = format!(
                    r#"
                    Add-Type -AssemblyName Microsoft.VisualBasic
                    [Microsoft.VisualBasic.Interaction]::AppActivate('{}')
                    "#,
                    Self::sanitize_powershell_string(title)?
                );
                self.run_powershell(&script).await
            }
            _ => Ok(ToolCallResult::error(format!("Unknown action: {}", action))),
        }
    }

    /// Execute PowerShell command on Windows
    #[cfg(target_os = "windows")]
    async fn windows_shell(&self, arguments: Value) -> Result<ToolCallResult> {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'command' parameter".to_string()))?;

        // Block command injection patterns: separators, subshells, piping to dangerous cmdlets
        let dangerous_patterns = [
            ";",
            "&&",
            "||", // Command separators
            "$(",
            "`", // Subshell/backtick injection
            "| Remove-",
            "| Stop-",
            "| Restart-", // Destructive piped cmdlets
            "Invoke-Expression",
            "iex ",
            "iex(", // Code execution
            "Start-Process",
            "New-Object System.Net.WebClient", // Process/network
            "-EncodedCommand",
            "-enc ", // Base64 encoded command bypass
        ];
        for pattern in &dangerous_patterns {
            if command.to_lowercase().contains(&pattern.to_lowercase()) {
                return Err(Error::InvalidInput(format!(
                    "Command blocked for safety: contains '{}'",
                    pattern
                )));
            }
        }

        self.run_powershell(command).await
    }

    /// Read clipboard on Windows
    #[cfg(target_os = "windows")]
    async fn windows_clipboard_read(&self) -> Result<ToolCallResult> {
        self.run_powershell("Get-Clipboard").await
    }

    /// Write to clipboard on Windows
    #[cfg(target_os = "windows")]
    async fn windows_clipboard_write(&self, arguments: Value) -> Result<ToolCallResult> {
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing 'content' parameter".to_string()))?;

        let safe_content = Self::sanitize_powershell_string(content)?;
        let script = format!("Set-Clipboard -Value '{}'", safe_content);
        self.run_powershell(&script).await
    }

    /// Helper to run PowerShell commands
    #[cfg(target_os = "windows")]
    async fn run_powershell(&self, script: &str) -> Result<ToolCallResult> {
        let output = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(ToolCallResult::text(
                json!({
                    "success": true,
                    "output": stdout.trim(),
                })
                .to_string(),
            ))
        } else {
            Ok(ToolCallResult::error(format!(
                "PowerShell error: {}",
                stderr
            )))
        }
    }
}

/// Escape special characters for SendKeys
#[cfg(target_os = "windows")]
fn escape_sendkeys(text: &str) -> String {
    text.replace('{', "{{}")
        .replace('}', "{}}")
        .replace('[', "{[}")
        .replace(']', "{]}")
        .replace('(', "{(}")
        .replace(')', "{)}")
        .replace('+', "{+}")
        .replace('^', "{^}")
        .replace('%', "{%}")
        .replace('~', "{~}")
}

/// Convert shortcut format (e.g., "ctrl+c") to SendKeys format
#[cfg(target_os = "windows")]
fn convert_shortcut_to_sendkeys(shortcut: &str) -> String {
    let parts: Vec<&str> = shortcut.split('+').collect();
    let mut result = String::new();

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => result.push('^'),
            "alt" => result.push('%'),
            "shift" => result.push('+'),
            key if is_last => {
                // Main key
                match key {
                    "enter" | "return" => result.push_str("{ENTER}"),
                    "tab" => result.push_str("{TAB}"),
                    "escape" | "esc" => result.push_str("{ESC}"),
                    "backspace" => result.push_str("{BACKSPACE}"),
                    "delete" | "del" => result.push_str("{DELETE}"),
                    "home" => result.push_str("{HOME}"),
                    "end" => result.push_str("{END}"),
                    "pageup" | "pgup" => result.push_str("{PGUP}"),
                    "pagedown" | "pgdn" => result.push_str("{PGDN}"),
                    "up" => result.push_str("{UP}"),
                    "down" => result.push_str("{DOWN}"),
                    "left" => result.push_str("{LEFT}"),
                    "right" => result.push_str("{RIGHT}"),
                    "f1" => result.push_str("{F1}"),
                    "f2" => result.push_str("{F2}"),
                    "f3" => result.push_str("{F3}"),
                    "f4" => result.push_str("{F4}"),
                    "f5" => result.push_str("{F5}"),
                    "f6" => result.push_str("{F6}"),
                    "f7" => result.push_str("{F7}"),
                    "f8" => result.push_str("{F8}"),
                    "f9" => result.push_str("{F9}"),
                    "f10" => result.push_str("{F10}"),
                    "f11" => result.push_str("{F11}"),
                    "f12" => result.push_str("{F12}"),
                    _ => result.push_str(key),
                }
            }
            _ => {}
        }
    }

    result
}

/// Tool definitions for platform automation
pub fn get_macos_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "osascript".to_string(),
            description: "Execute AppleScript commands to control Mac applications".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "The AppleScript code to execute"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30)",
                        "default": 30
                    }
                },
                "required": ["script"]
            }),
        },
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "screenshot".to_string(),
            description: "Capture a screenshot of the screen or a region".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to save the screenshot (default: ~/Library/Caches/canal/screenshots/screenshot.png on macOS)"
                    },
                    "region": {
                        "type": "object",
                        "description": "Optional region to capture",
                        "properties": {
                            "x": {"type": "integer"},
                            "y": {"type": "integer"},
                            "width": {"type": "integer"},
                            "height": {"type": "integer"}
                        }
                    }
                }
            }),
        },
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "app_control".to_string(),
            description: "Control Mac applications (launch, quit, hide, show, minimize)"
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["launch", "quit", "hide", "show", "minimize"],
                        "description": "The action to perform"
                    },
                    "app": {
                        "type": "string",
                        "description": "The application name"
                    }
                },
                "required": ["action", "app"]
            }),
        },
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "open_url".to_string(),
            description: "Open a URL in the default browser".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to open"
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "notify".to_string(),
            description: "Show a macOS notification".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Notification title"
                    },
                    "message": {
                        "type": "string",
                        "description": "Notification message"
                    }
                },
                "required": ["message"]
            }),
        },
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "clipboard_read".to_string(),
            description: "Read the current clipboard content".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "clipboard_write".to_string(),
            description: "Write content to the clipboard".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Content to copy to clipboard"
                    }
                },
                "required": ["content"]
            }),
        },
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "get_frontmost_app".to_string(),
            description: "Get the name of the currently active application".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            namespace: "mac".to_string(),
            name: "list_running_apps".to_string(),
            description: "List all running applications".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}

pub fn get_windows_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            namespace: "win".to_string(),
            name: "click".to_string(),
            description: "Click at screen coordinates".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "X coordinate"},
                    "y": {"type": "integer", "description": "Y coordinate"},
                    "button": {"type": "string", "enum": ["left", "right", "middle"], "default": "left"},
                    "click_type": {"type": "string", "enum": ["single", "double"], "default": "single"}
                },
                "required": ["x", "y"]
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "type_text".to_string(),
            description: "Type text at the current cursor position".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to type"},
                    "clear": {"type": "boolean", "description": "Clear existing text first", "default": false}
                },
                "required": ["text"]
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "scroll".to_string(),
            description: "Scroll the mouse wheel".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "direction": {"type": "string", "enum": ["up", "down"], "default": "down"},
                    "amount": {"type": "integer", "description": "Scroll notches", "default": 3}
                }
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "move_mouse".to_string(),
            description: "Move the mouse cursor".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "X coordinate"},
                    "y": {"type": "integer", "description": "Y coordinate"}
                },
                "required": ["x", "y"]
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "shortcut".to_string(),
            description: "Execute a keyboard shortcut (e.g., 'ctrl+c')".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "keys": {"type": "string", "description": "Shortcut (e.g., 'ctrl+shift+s')"}
                },
                "required": ["keys"]
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "wait".to_string(),
            description: "Pause execution".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "duration_ms": {"type": "integer", "description": "Duration in ms", "default": 1000}
                }
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "snapshot".to_string(),
            description: "Capture a screenshot".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Save path", "default": "C:\\temp\\screenshot.png"}
                }
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "app".to_string(),
            description: "Control Windows applications".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["launch", "focus"]},
                    "app": {"type": "string", "description": "App path (for launch)"},
                    "title": {"type": "string", "description": "Window title (for focus)"}
                },
                "required": ["action"]
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "shell".to_string(),
            description: "Execute a PowerShell command".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "PowerShell command"}
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "clipboard_read".to_string(),
            description: "Read clipboard content".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDefinition {
            namespace: "win".to_string(),
            name: "clipboard_write".to_string(),
            description: "Write to clipboard".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string", "description": "Content to copy"}
                },
                "required": ["content"]
            }),
        },
    ]
}

/// Tool definition structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        let automation = PlatformAutomation::new();

        #[cfg(target_os = "macos")]
        {
            assert_eq!(automation.namespace(), "mac");
            assert!(automation.is_supported());
        }

        #[cfg(target_os = "windows")]
        {
            assert_eq!(automation.namespace(), "win");
            assert!(automation.is_supported());
        }
    }

    #[test]
    fn test_tool_definitions() {
        let mac_tools = get_macos_tool_definitions();
        assert!(!mac_tools.is_empty());
        assert!(mac_tools.iter().any(|t| t.name == "osascript"));

        let win_tools = get_windows_tool_definitions();
        assert!(!win_tools.is_empty());
        assert!(win_tools.iter().any(|t| t.name == "click"));
    }
}
