//! Assistant system-control tools (opt-in): clipboard, window management,
//! and screen capture. File tools reuse the project toolset over configured
//! roots. Everything here is approval-gated.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::config::AssistantConfig;
use harness::permissions::ApprovalKey;
use harness::tools::Tool;

/// Tools enabled by the assistant's system-control settings.
pub fn tools(config: &AssistantConfig) -> Vec<Arc<dyn Tool>> {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    if config.tool_clipboard {
        out.push(Arc::new(ClipboardTool));
    }
    if config.tool_windows {
        out.push(Arc::new(WindowTool));
    }
    if config.tool_screen {
        out.push(Arc::new(ScreenTool));
    }
    out
}

fn key(tool: &'static str, command: Option<&str>) -> Option<ApprovalKey> {
    Some(ApprovalKey {
        tool: tool.to_string(),
        command: command.map(str::to_string),
    })
}

// ── Clipboard ─────────────────────────────────────────────────────────────

pub struct ClipboardTool;

impl Tool for ClipboardTool {
    fn name(&self) -> String {
        "clipboard".to_string()
    }

    fn description(&self) -> String {
        "Read or write the system clipboard. `read` returns the text; `write` replaces it \
         (needs approval)."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["read", "write"] },
                "text": { "type": "string", "description": "Text to copy (write)." }
            },
            "required": ["action"]
        })
    }

    fn approval_key(&self, args: &Value) -> Option<ApprovalKey> {
        match args.get("action").and_then(Value::as_str) {
            Some("write") => key("clipboard", Some("write")),
            _ => None,
        }
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "read" => {
                let mut clip = arboard::Clipboard::new().map_err(|e| anyhow!(e.to_string()))?;
                match clip.get_text() {
                    Ok(text) => Ok(text),
                    Err(_) => Ok("(clipboard is empty or not text)".to_string()),
                }
            }
            "write" => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("'text' is required for write"))?;
                let mut clip = arboard::Clipboard::new().map_err(|e| anyhow!(e.to_string()))?;
                clip.set_text(text.to_string())
                    .map_err(|e| anyhow!(e.to_string()))?;
                Ok(format!("Copied {} characters.", text.chars().count()))
            }
            _ => bail!("Unknown action '{action}'"),
        }
    }
}

// ── Windows ───────────────────────────────────────────────────────────────

pub struct WindowTool;

impl Tool for WindowTool {
    fn name(&self) -> String {
        "window".to_string()
    }

    fn description(&self) -> String {
        "Manage desktop windows. `list` shows visible windows; `focus`, `minimize`, `maximize`, \
         `restore`, `close`, and `move` take a `title` substring (first match). Mutating actions \
         need approval."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "focus", "minimize", "maximize", "restore", "close", "move"] },
                "title": { "type": "string", "description": "Case-insensitive window title substring." },
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "width": { "type": "integer" },
                "height": { "type": "integer" }
            },
            "required": ["action"]
        })
    }

    fn approval_key(&self, args: &Value) -> Option<ApprovalKey> {
        match args.get("action").and_then(Value::as_str) {
            Some("list") | None => None,
            Some(action) => key("window", Some(action)),
        }
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        if action == "list" {
            return list_windows();
        }
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow!("'title' is required for {action}"))?;
        #[cfg(target_os = "windows")]
        {
            windows_action(action, title, args)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (action, title, args);
            bail!("Window control is only available on Windows for now")
        }
    }
}

#[cfg(target_os = "windows")]
fn list_windows() -> Result<String> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible};

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<(isize, String)>);
        if IsWindowVisible(hwnd).as_bool() {
            let mut buf = [0u16; 512];
            let len = GetWindowTextW(hwnd, &mut buf);
            if len > 0 {
                out.push((hwnd.0 as isize, String::from_utf16_lossy(&buf[..len as usize])));
            }
        }
        BOOL(1)
    }

    let mut found: Vec<(isize, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut _ as isize));
    }
    if found.is_empty() {
        return Ok("No visible windows.".to_string());
    }
    let mut text = String::new();
    for (i, (_, title)) in found.iter().take(50).enumerate() {
        text.push_str(&format!("{}. {title}\n", i + 1));
    }
    Ok(text)
}

#[cfg(target_os = "windows")]
fn find_window(title: &str) -> Result<isize> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible};

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<(isize, String)>);
        if IsWindowVisible(hwnd).as_bool() {
            let mut buf = [0u16; 512];
            let len = GetWindowTextW(hwnd, &mut buf);
            if len > 0 {
                out.push((hwnd.0 as isize, String::from_utf16_lossy(&buf[..len as usize])));
            }
        }
        BOOL(1)
    }

    let mut found: Vec<(isize, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut _ as isize));
    }
    let needle = title.to_lowercase();
    found
        .into_iter()
        .find(|(_, t)| t.to_lowercase().contains(&needle))
        .map(|(h, _)| h)
        .ok_or_else(|| anyhow!("No visible window matching '{title}'"))
}

#[cfg(target_os = "windows")]
fn windows_action(action: &str, title: &str, args: &Value) -> Result<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        PostMessageW, SetForegroundWindow, SetWindowPos, ShowWindow, SW_MAXIMIZE, SW_MINIMIZE,
        SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, WM_CLOSE,
    };

    let raw = find_window(title)?;
    let hwnd = HWND(raw as *mut _);
    unsafe {
        match action {
            "focus" => {
                let _ = SetForegroundWindow(hwnd);
                Ok(format!("Focused '{title}'."))
            }
            "minimize" => {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
                Ok(format!("Minimized '{title}'."))
            }
            "maximize" => {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                Ok(format!("Maximized '{title}'."))
            }
            "restore" => {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                Ok(format!("Restored '{title}'."))
            }
            "close" => {
                PostMessageW(Some(hwnd), WM_CLOSE, Default::default(), Default::default())
                    .map_err(|e| anyhow!(e.to_string()))?;
                Ok(format!("Asked '{title}' to close."))
            }
            "move" => {
                let num = |name: &str| {
                    args.get(name)
                        .and_then(Value::as_i64)
                        .ok_or_else(|| anyhow!("'{name}' is required for move"))
                };
                let (x, y, w, h) = (num("x")? as i32, num("y")? as i32, num("width")? as i32, num("height")? as i32);
                SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE)
                    .map_err(|e| anyhow!(e.to_string()))?;
                Ok(format!("Moved '{title}' to {x},{y} ({w}x{h})."))
            }
            _ => bail!("Unknown action '{action}'"),
        }
    }
}

// ── Screen capture ────────────────────────────────────────────────────────

pub struct ScreenTool;

impl Tool for ScreenTool {
    fn name(&self) -> String {
        "screen".to_string()
    }

    fn description(&self) -> String {
        "Capture a monitor (`target: monitor`, `index`) or a window (`target: window`, \
         `title`) to a PNG under the assistant data folder and return its path. Needs approval."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": { "type": "string", "enum": ["monitor", "window"], "description": "Default monitor." },
                "index": { "type": "integer", "description": "Monitor index (default 0)." },
                "title": { "type": "string", "description": "Window title substring (window target)." }
            },
            "required": []
        })
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        key("screen", Some("capture"))
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let target = args.get("target").and_then(Value::as_str).unwrap_or("monitor");
        let dir = crate::assistant::dir()
            .ok_or_else(|| anyhow!("Cannot find data directory"))?
            .join("screens");
        std::fs::create_dir_all(&dir)?;
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let path = dir.join(format!("capture-{millis}.png"));

        let (width, height) = match target {
            "monitor" => {
                let monitors = xcap::Monitor::all().map_err(|e| anyhow!(e.to_string()))?;
                let index = args.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let monitor = monitors
                    .get(index)
                    .ok_or_else(|| anyhow!("No monitor with index {index}"))?;
                let image = monitor.capture_image().map_err(|e| anyhow!(e.to_string()))?;
                let (w, h) = (image.width(), image.height());
                write_png(&path, w, h, image.as_raw())?;
                (w, h)
            }
            "window" => {
                let title = args
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| anyhow!("'title' is required for a window capture"))?;
                let needle = title.to_lowercase();
                let windows = xcap::Window::all().map_err(|e| anyhow!(e.to_string()))?;
                let window = windows
                    .into_iter()
                    .find(|w| {
                        w.title()
                            .map(|t| t.to_lowercase().contains(&needle))
                            .unwrap_or(false)
                    })
                    .ok_or_else(|| anyhow!("No window matching '{title}'"))?;
                let image = window.capture_image().map_err(|e| anyhow!(e.to_string()))?;
                let (w, h) = (image.width(), image.height());
                write_png(&path, w, h, image.as_raw())?;
                (w, h)
            }
            other => bail!("Unknown target '{other}'"),
        };
        Ok(format!("Saved {width}x{height} capture to {}", path.display()))
    }
}

/// Minimal PNG writer (8-bit RGBA, no interlace) so captures do not need the
/// image crate.
fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    use std::io::Write;
    let row = width as usize * 4;
    if rgba.len() < row * height as usize {
        bail!("Capture buffer is too small");
    }
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * row..(y + 1) * row]);
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&raw)?;
    let idat = encoder.finish()?;

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    std::fs::write(path, out)?;
    Ok(())
}

/// The assistant's configured file roots that still exist.
pub fn file_roots(config: &AssistantConfig) -> Vec<PathBuf> {
    config
        .roots
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .take(16)
        .collect()
}
