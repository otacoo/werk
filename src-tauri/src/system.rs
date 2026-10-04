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
    if config.tool_input {
        out.push(Arc::new(InputTool));
    }
    if config.tool_browser {
        out.push(Arc::new(crate::browser::BrowserTool::new(
            config.browser_user_profile,
        )));
    }
    out
}

// System tools never ask for approval: the jail and the File system roots
// bound what they can reach instead.

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

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
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

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
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

// ── Input (mouse + keyboard) ──────────────────────────────────────────────

pub struct InputTool;

impl Tool for InputTool {
    fn name(&self) -> String {
        "input".to_string()
    }

    fn description(&self) -> String {
        "Control the mouse and keyboard. `move` (x, y), `click` (button, optional x/y, `double`), \
         `drag` (from_x/from_y/to_x/to_y, button), `scroll` (amount; positive = down, optional \
         horizontal), `type` (text), `key` (keys: modifiers then the key, e.g. [\"ctrl\",\"c\"]). \
         Coordinates are absolute screen pixels. Every action needs approval."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["move", "click", "drag", "scroll", "type", "key"] },
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "from_x": { "type": "integer" },
                "from_y": { "type": "integer" },
                "to_x": { "type": "integer" },
                "to_y": { "type": "integer" },
                "button": { "type": "string", "enum": ["left", "right", "middle"] },
                "double": { "type": "boolean" },
                "amount": { "type": "integer", "description": "Scroll clicks; positive = down." },
                "horizontal": { "type": "integer", "description": "Optional horizontal scroll." },
                "text": { "type": "string" },
                "keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Modifiers then the key, e.g. [\"ctrl\", \"c\"]."
                }
            },
            "required": ["action"]
        })
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }

    fn execute(&self, args: &Value) -> Result<String> {
        use enigo::{Axis, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        let mut enigo = Enigo::new(&Settings::default()).map_err(|e| anyhow!("Cannot control input: {e}"))?;
        let err = |e: enigo::InputError| anyhow!(e.to_string());
        match action {
            "move" => {
                let (x, y) = (int_arg(args, "x")?, int_arg(args, "y")?);
                enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
                Ok(format!("Moved the pointer to {x},{y}."))
            }
            "click" => {
                if let (Some(x), Some(y)) = (opt_int(args, "x"), opt_int(args, "y")) {
                    enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
                }
                let button = parse_button(args.get("button").and_then(Value::as_str))?;
                let double = args.get("double").and_then(Value::as_bool).unwrap_or(false);
                enigo.button(button, Direction::Click).map_err(err)?;
                if double {
                    enigo.button(button, Direction::Click).map_err(err)?;
                }
                Ok(format!(
                    "{} {button:?} click.",
                    if double { "Double" } else { "Single" }
                ))
            }
            "drag" => {
                let (fx, fy) = (int_arg(args, "from_x")?, int_arg(args, "from_y")?);
                let (tx, ty) = (int_arg(args, "to_x")?, int_arg(args, "to_y")?);
                let button = parse_button(args.get("button").and_then(Value::as_str))?;
                enigo.move_mouse(fx, fy, Coordinate::Abs).map_err(err)?;
                enigo.button(button, Direction::Press).map_err(err)?;
                enigo.move_mouse(tx, ty, Coordinate::Abs).map_err(err)?;
                enigo.button(button, Direction::Release).map_err(err)?;
                Ok(format!("Dragged from {fx},{fy} to {tx},{ty}."))
            }
            "scroll" => {
                let amount = int_arg(args, "amount")?;
                enigo.scroll(amount, Axis::Vertical).map_err(err)?;
                if let Some(horizontal) = opt_int(args, "horizontal") {
                    enigo.scroll(horizontal, Axis::Horizontal).map_err(err)?;
                }
                Ok(format!("Scrolled {amount}."))
            }
            "type" => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| anyhow!("'text' is required for type"))?;
                let capped: String = text.chars().take(5_000).collect();
                let count = capped.chars().count();
                enigo.text(&capped).map_err(err)?;
                Ok(format!("Typed {count} characters."))
            }
            "key" => {
                let names: Vec<String> = args
                    .get("keys")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if names.is_empty() {
                    bail!("'keys' must list at least one key");
                }
                let keys: Vec<enigo::Key> =
                    names.iter().map(|n| parse_key(n)).collect::<Result<_>>()?;
                let (last, modifiers) = keys.split_last().expect("non-empty");
                for m in modifiers {
                    enigo.key(*m, Direction::Press).map_err(err)?;
                }
                enigo.key(*last, Direction::Click).map_err(err)?;
                for m in modifiers.iter().rev() {
                    enigo.key(*m, Direction::Release).map_err(err)?;
                }
                Ok(format!("Pressed {}.", names.join("+")))
            }
            _ => bail!("Unknown action '{action}'"),
        }
    }
}

fn int_arg(args: &Value, name: &str) -> Result<i32> {
    args.get(name)
        .and_then(Value::as_i64)
        .map(|v| v as i32)
        .ok_or_else(|| anyhow!("'{name}' is required"))
}

fn opt_int(args: &Value, name: &str) -> Option<i32> {
    args.get(name).and_then(Value::as_i64).map(|v| v as i32)
}

fn parse_button(name: Option<&str>) -> Result<enigo::Button> {
    match name.unwrap_or("left") {
        "left" => Ok(enigo::Button::Left),
        "right" => Ok(enigo::Button::Right),
        "middle" => Ok(enigo::Button::Middle),
        other => bail!("Unknown button '{other}'"),
    }
}

fn parse_key(name: &str) -> Result<enigo::Key> {
    use enigo::Key;
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();
    let key = match lower.as_str() {
        "ctrl" | "control" => Key::Control,
        "shift" => Key::Shift,
        "alt" => Key::Alt,
        "meta" | "win" | "windows" | "super" | "cmd" | "command" => Key::Meta,
        "enter" | "return" => Key::Return,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        _ => {
            if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                match n {
                    1 => Key::F1,
                    2 => Key::F2,
                    3 => Key::F3,
                    4 => Key::F4,
                    5 => Key::F5,
                    6 => Key::F6,
                    7 => Key::F7,
                    8 => Key::F8,
                    9 => Key::F9,
                    10 => Key::F10,
                    11 => Key::F11,
                    12 => Key::F12,
                    _ => bail!("Unknown key '{name}'"),
                }
            } else {
                let mut chars = trimmed.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Key::Unicode(c),
                    _ => bail!("Unknown key '{name}'"),
                }
            }
        }
    };
    Ok(key)
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
        None
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let target = args.get("target").and_then(Value::as_str).unwrap_or("monitor");
        let dir = temp_workspace().join("screens");
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

    fn result_images(&self, _args: &Value, output: &str) -> Vec<(String, Vec<u8>)> {
        // The output ends with the saved path.
        let Some(path) = output.rsplit(' ').next() else {
            return Vec::new();
        };
        harness::tools::image_attachment(std::path::Path::new(path))
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

/// Ephemeral workspace: always granted, safe to delete (screenshots, scratch
/// scripts, downloads).
pub fn temp_workspace() -> PathBuf {
    std::env::temp_dir().join("werk-assistant")
}

/// The assistant's persistent home folder, when configured and present.
pub fn home(config: &AssistantConfig) -> Option<PathBuf> {
    config
        .workspace
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// File-tool roots: the assistant home first (relative paths land there),
/// then the temp workspace when enabled, then the extra folders. Everything
/// outside this list is barred by the jail.
pub fn roots(config: &AssistantConfig) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home(config) {
        roots.push(home);
    }
    if config.temp_enabled {
        let temp = temp_workspace();
        let _ = std::fs::create_dir_all(&temp);
        roots.push(temp);
    }
    for folder in &config.folders {
        let path = PathBuf::from(folder);
        if path.is_dir() && !roots.iter().any(|r| r == &path) {
            roots.push(path);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use enigo::Key;

    #[test]
    fn parses_common_keys_and_buttons() {
        assert!(matches!(parse_key("ctrl").unwrap(), Key::Control));
        assert!(matches!(parse_key("F5").unwrap(), Key::F5));
        assert!(matches!(parse_key("enter").unwrap(), Key::Return));
        assert!(matches!(parse_key("c").unwrap(), Key::Unicode('c')));
        assert!(parse_key("nope!").is_err());
        assert!(matches!(parse_button(Some("right")).unwrap(), enigo::Button::Right));
        assert!(matches!(parse_button(None).unwrap(), enigo::Button::Left));
        assert!(parse_button(Some("weird")).is_err());
    }

    #[test]
    fn roots_follow_the_file_system_settings() {
        let mut config = AssistantConfig::default();
        config.temp_enabled = false;
        assert!(roots(&config).is_empty());
        config.temp_enabled = true;
        let with_temp = roots(&config);
        assert_eq!(with_temp.len(), 1);
        assert!(with_temp[0].ends_with("werk-assistant"));
        // Missing folders are skipped; existing ones append.
        config.folders = vec!["Z:\\definitely-missing".to_string(), std::env::temp_dir().to_string_lossy().to_string()];
        let with_folder = roots(&config);
        assert_eq!(with_folder.len(), 2);
        assert_eq!(with_folder[0], std::env::temp_dir().join("werk-assistant"));
        assert_eq!(with_folder[1], std::env::temp_dir());
    }
}
