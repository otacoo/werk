//! Assistant profile: personality prompt, memory file, and prompt assembly.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::config::{AppConfig, AssistantConfig};

/// Built-in assistant prompt: identity, behavior, and tool etiquette. Always
/// sent first, before the persona, memory, and skills layers.
pub const BUILT_IN_ASSISTANT_PROMPT: &str = "\
You are {{name}}, the user's personal assistant. You are not a generic chatbot: you have a \
personality, opinions, and continuity. You remember what matters about the user across \
conversations, and you act rather than narrate.

Behavior:
- Be direct and useful. Answer the question, then add only what helps.
- Have a personality: warmth, humor, honest opinions. Disagree when the user is wrong.
- Never pretend to know something you do not; say so and offer to find out.
- Keep replies short unless the task needs depth.

Tools:
- Use the tools you have instead of describing what the user could do.
- remember stores durable facts about the user and their preferences; use it whenever you \
learn something lasting. It stays hidden, so never mention it.
- skill loads a saved procedure by name when it matches the task.
- web_search looks up current facts, releases, and docs; include the URLs you used.
- File tools work only in the locations listed under Files below; everything else is blocked.
- ask_user (2-4 options) when a decision is genuinely the user's.
- get_time for the current date or time when it matters.
- When a request is ambiguous, ask one focused question instead of guessing.";

pub fn dir() -> Option<PathBuf> {
    crate::config::data_dir().map(|d| d.join("werk").join("assistant"))
}

pub fn memory_path() -> Option<PathBuf> {
    dir().map(|d| d.join("MEMORY.md"))
}

pub fn sessions_dir() -> Result<PathBuf, String> {
    dir()
        .map(|d| d.join("sessions"))
        .ok_or_else(|| "Cannot find data directory".to_string())
}

/// System prompt: base (built-in or custom), persona, date, memory, skills.
pub fn system_prompt(config: &AppConfig, skills: &[harness::skills::Skill]) -> String {
    let custom = config
        .assistant
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let mut base = match custom {
        Some(text) => text.to_string(),
        None => BUILT_IN_ASSISTANT_PROMPT.to_string(),
    };
    base = base
        .replace("{{name}}", config.assistant.name.trim())
        .replace("{{user}}", "the user");
    if !config.assistant.persona.trim().is_empty() {
        base.push_str("\n\n## Personality\n");
        base.push_str(config.assistant.persona.trim());
    }
    base.push_str("\n\nCurrent date: ");
    base.push_str(&harness::tools::TimeTool::now_utc());
    base.push_str(&files_block(config));
    if let Some(dir) = dir() {
        base.push_str(&harness::memory::load_block(memory_path().as_deref(), &dir));
    }
    base.push_str(&harness::skills::system_prompt_listing(skills));
    base
}

/// The only folders the file tools may touch, plus where relative paths go.
fn files_block(config: &AppConfig) -> String {
    let a = &config.assistant;
    let workspace = a
        .workspace
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty());
    let mut lines: Vec<String> = Vec::new();
    if let Some(w) = workspace {
        lines.push(format!("- Your folder: {w}"));
    }
    if a.temp_enabled {
        lines.push(format!(
            "- Temp workspace (scratch files and screenshots; safe to delete): {}",
            crate::system::temp_workspace().display()
        ));
    }
    for folder in &a.folders {
        let folder = folder.trim();
        if !folder.is_empty() {
            lines.push(format!("- Accessible folder: {folder}"));
        }
    }
    if lines.is_empty() {
        return "\n\n## Files\nNo file locations are configured, so the file tools are unavailable."
            .to_string();
    }
    let relative = match (workspace, a.temp_enabled) {
        (Some(_), _) => "Relative paths resolve in your folder.",
        (None, true) => "Relative paths resolve in the temp workspace.",
        (None, false) => "Use absolute paths.",
    };
    format!(
        "\n\n## Files\nOnly these locations can be read or written; everything else is blocked. \
         {relative}\n{}",
        lines.join("\n")
    )
}

/// Global skills the assistant may load on demand.
pub fn skills() -> Vec<harness::skills::Skill> {
    let roots: Vec<PathBuf> = crate::config::data_dir()
        .map(|d| vec![d.join("werk").join("skills")])
        .unwrap_or_default();
    harness::skills::discover(&roots)
}

/// Save assistant identity, persona, prompt override, and sampling.
#[tauri::command]
#[specta::specta]
pub async fn set_assistant_config(
    config: AssistantConfig,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let cap = |s: String, n: usize| s.chars().take(n).collect::<String>();
    let mut cfg = state.config.lock().unwrap();
    cfg.assistant.name = cap(config.name.trim().to_string(), 60);
    if cfg.assistant.name.is_empty() {
        cfg.assistant.name = "Werk".to_string();
    }
    cfg.assistant.persona = cap(config.persona.trim().to_string(), 8_000);
    cfg.assistant.system_prompt = config
        .system_prompt
        .map(|p| cap(p.trim().to_string(), 20_000))
        .filter(|p| !p.is_empty());
    cfg.assistant.temperature = config.temperature.filter(|t| (0.0..=2.0).contains(t));
    cfg.assistant.top_p = config.top_p.filter(|p| (0.0..=1.0).contains(p));
    cfg.assistant.repeat_penalty = config.repeat_penalty.filter(|p| (0.0..=2.0).contains(p));
    cfg.assistant.reasoning_effort = config
        .reasoning_effort
        .map(|e| cap(e.trim().to_string(), 32))
        .filter(|e| !e.is_empty());
    cfg.save().map_err(|e| e.to_string())
}

/// Always-on behavior: alerts, proactive turns, autostart, overlay, hotkey.
/// Tray behavior follows the shared `close_to_tray` setting.
#[tauri::command]
#[specta::specta]
pub async fn set_assistant_behavior(
    app: tauri::AppHandle,
    notify: bool,
    proactive: bool,
    autostart: bool,
    overlay_enabled: bool,
    hotkey: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    use tauri::Manager;
    use tauri_plugin_autostart::ManagerExt;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let hotkey: String = hotkey.trim().chars().take(64).collect();
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.assistant.notify = notify;
        cfg.assistant.proactive = proactive;
        cfg.assistant.autostart = autostart;
        cfg.assistant.overlay_enabled = overlay_enabled;
        cfg.assistant.hotkey = hotkey.clone();
        cfg.save().map_err(|e| e.to_string())?;
    }
    let launch = app.autolaunch();
    let _ = if autostart { launch.enable() } else { launch.disable() };
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    if overlay_enabled && !hotkey.is_empty() {
        shortcuts
            .register(hotkey.as_str())
            .map_err(|e| format!("Invalid hotkey: {e}"))?;
    }
    // Disabling the overlay hides it right away.
    if !overlay_enabled {
        if let Some(win) = app.get_webview_window("overlay") {
            let _ = win.hide();
        }
    }
    Ok(())
}

/// Built-in assistant prompt for the Persona editor.
#[tauri::command]
#[specta::specta]
pub async fn assistant_system_prompt_default() -> Result<String, String> {
    Ok(BUILT_IN_ASSISTANT_PROMPT.to_string())
}

/// Import (or clear) the assistant's profile image.
#[tauri::command]
#[specta::specta]
pub async fn assistant_set_avatar(
    path: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let dir = dir()
        .ok_or_else(|| "Cannot find data directory".to_string())?
        .join("avatars");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut cfg = state.config.lock().unwrap();
    match path {
        Some(p) => {
            let src = Path::new(&p);
            let ext = src
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif"))
                .unwrap_or_else(|| "png".to_string());
            for old in ["png", "jpg", "jpeg", "webp", "gif"] {
                let _ = std::fs::remove_file(dir.join(format!("avatar.{old}")));
            }
            let target = dir.join(format!("avatar.{ext}"));
            let bytes = std::fs::read(src).map_err(|e| format!("Cannot read avatar: {e}"))?;
            std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
            cfg.assistant.avatar = Some(target.to_string_lossy().to_string());
        }
        None => {
            for old in ["png", "jpg", "jpeg", "webp", "gif"] {
                let _ = std::fs::remove_file(dir.join(format!("avatar.{old}")));
            }
            cfg.assistant.avatar = None;
        }
    }
    cfg.save().map_err(|e| e.to_string())
}

/// The assistant's avatar as a data URL, when one is set.
#[tauri::command]
#[specta::specta]
pub async fn assistant_avatar(
    state: State<'_, crate::AppState>,
) -> Result<Option<String>, String> {
    let path = state.config.lock().unwrap().assistant.avatar.clone();
    let Some(path) = path else {
        return Ok(None);
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    let mime = match Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    };
    Ok(Some(format!(
        "data:{mime};base64,{}",
        crate::roleplay::encode_base64(&bytes)
    )))
}

/// Save the assistant's system-control settings (master switch + tool
/// toggles). File locations live in `set_assistant_fs`.
#[tauri::command]
#[specta::specta]
pub async fn set_assistant_access(
    system_control: bool,
    files: bool,
    clipboard: bool,
    windows: bool,
    screen: bool,
    input: bool,
    browser: bool,
    browser_user_profile: bool,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    cfg.assistant.system_control = system_control;
    cfg.assistant.tool_files = files;
    cfg.assistant.tool_clipboard = clipboard;
    cfg.assistant.tool_windows = windows;
    cfg.assistant.tool_screen = screen;
    cfg.assistant.tool_input = input;
    cfg.assistant.tool_browser = browser;
    cfg.assistant.browser_user_profile = browser_user_profile;
    cfg.save().map_err(|e| e.to_string())
}

/// Save the assistant's file locations. Everything outside these is barred.
#[tauri::command]
#[specta::specta]
pub async fn set_assistant_fs(
    workspace: Option<String>,
    temp_enabled: bool,
    folders: Vec<String>,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let workspace = workspace
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty() && std::path::Path::new(w).is_dir());
    let mut cleaned: Vec<String> = Vec::new();
    for folder in folders {
        let folder = folder.trim();
        if folder.is_empty()
            || cleaned.iter().any(|f| f.eq_ignore_ascii_case(folder))
            || workspace.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(folder))
        {
            continue;
        }
        if std::path::Path::new(folder).is_dir() {
            cleaned.push(folder.to_string());
        }
        if cleaned.len() >= 16 {
            break;
        }
    }
    let mut cfg = state.config.lock().unwrap();
    cfg.assistant.workspace = workspace;
    cfg.assistant.temp_enabled = temp_enabled;
    cfg.assistant.folders = cleaned;
    cfg.save().map_err(|e| e.to_string())
}

/// The temp workspace path, for the File system card.
#[tauri::command]
#[specta::specta]
pub async fn assistant_temp_dir() -> Result<String, String> {
    Ok(crate::system::temp_workspace().to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_layers_base_persona_and_name() {
        let mut config = AppConfig::default();
        config.assistant.name = "Ada".into();
        config.assistant.persona = "Dry wit.".into();
        let out = system_prompt(&config, &[]);
        assert!(out.contains("You are Ada"), "{out}");
        assert!(out.contains("## Personality\nDry wit."), "{out}");
        assert!(out.contains("Current date: "), "{out}");

        // A custom prompt replaces the built-in but keeps the persona.
        config.assistant.system_prompt = Some("Custom for {{name}}.".into());
        let out = system_prompt(&config, &[]);
        assert!(out.starts_with("Custom for Ada."), "{out}");
        assert!(!out.contains("personal assistant"), "{out}");
        assert!(out.contains("Dry wit."), "{out}");

        // The Files block names every location the tools may touch.
        config.assistant.workspace = Some("D:\\Keep".into());
        config.assistant.folders = vec!["E:\\Shared".into()];
        let out = system_prompt(&config, &[]);
        assert!(out.contains("Your folder: D:\\Keep"), "{out}");
        assert!(out.contains("Accessible folder: E:\\Shared"), "{out}");
        assert!(out.contains("Temp workspace"), "{out}");
        assert!(out.contains("everything else is blocked"), "{out}");

        // No locations: file tools are called out as unavailable.
        config.assistant.workspace = None;
        config.assistant.folders.clear();
        config.assistant.temp_enabled = false;
        let out = system_prompt(&config, &[]);
        assert!(out.contains("file tools are unavailable"), "{out}");
    }
}
