//! Assistant profile: personality prompt, memory file, and prompt assembly.

use std::path::PathBuf;

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
    if let Some(dir) = dir() {
        base.push_str(&harness::memory::load_block(memory_path().as_deref(), &dir));
    }
    base.push_str(&harness::skills::system_prompt_listing(skills));
    base
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

/// Always-on behavior: alerts, proactive turns, autostart, hotkey. Tray
/// behavior follows the shared `close_to_tray` setting.
#[tauri::command]
#[specta::specta]
pub async fn set_assistant_behavior(
    app: tauri::AppHandle,
    notify: bool,
    proactive: bool,
    autostart: bool,
    hotkey: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let hotkey: String = hotkey.trim().chars().take(64).collect();
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.assistant.notify = notify;
        cfg.assistant.proactive = proactive;
        cfg.assistant.autostart = autostart;
        cfg.assistant.hotkey = hotkey.clone();
        cfg.save().map_err(|e| e.to_string())?;
    }
    let launch = app.autolaunch();
    let _ = if autostart { launch.enable() } else { launch.disable() };
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    if !hotkey.is_empty() {
        shortcuts
            .register(hotkey.as_str())
            .map_err(|e| format!("Invalid hotkey: {e}"))?;
    }
    Ok(())
}

/// Built-in assistant prompt for the Persona editor.
#[tauri::command]
#[specta::specta]
pub async fn assistant_system_prompt_default() -> Result<String, String> {
    Ok(BUILT_IN_ASSISTANT_PROMPT.to_string())
}

/// Save the assistant's system-control settings (master switch, roots, tools).
#[tauri::command]
#[specta::specta]
pub async fn set_assistant_access(
    system_control: bool,
    roots: Vec<String>,
    files: bool,
    clipboard: bool,
    windows: bool,
    screen: bool,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let mut cleaned: Vec<String> = Vec::new();
    for root in roots {
        let root = root.trim();
        if root.is_empty() || cleaned.iter().any(|r| r.eq_ignore_ascii_case(root)) {
            continue;
        }
        if std::path::Path::new(root).is_dir() {
            cleaned.push(root.to_string());
        }
        if cleaned.len() >= 16 {
            break;
        }
    }
    let mut cfg = state.config.lock().unwrap();
    cfg.assistant.system_control = system_control;
    cfg.assistant.roots = cleaned;
    cfg.assistant.tool_files = files;
    cfg.assistant.tool_clipboard = clipboard;
    cfg.assistant.tool_windows = windows;
    cfg.assistant.tool_screen = screen;
    cfg.save().map_err(|e| e.to_string())
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
    }
}
