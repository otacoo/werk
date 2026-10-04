//! Assistant reminders: a persisted store, a free `reminder` tool, and the
//! background tick that fires notifications and proactive turns.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ReminderKind {
    /// System notification only.
    Notify,
    /// Notification plus a proactive assistant turn when it fires.
    Message,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct Reminder {
    pub id: String,
    pub text: String,
    /// Unix seconds.
    pub due: u32,
    /// Repeat interval; None = one-shot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_secs: Option<u32>,
    pub kind: ReminderKind,
    pub done: bool,
}

fn store_path() -> Option<PathBuf> {
    crate::assistant::dir().map(|d| d.join("reminders.json"))
}

pub fn load() -> Vec<Reminder> {
    store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(store: &[Reminder]) -> Result<(), String> {
    let path = store_path().ok_or_else(|| "Cannot find data directory".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn new_id(store: &[Reminder]) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut id = format!("r{millis}");
    while store.iter().any(|r| r.id == id) {
        id.push('x');
    }
    id
}

/// Free tool: the assistant schedules and curates its own reminders.
pub struct ReminderTool;

impl harness::tools::Tool for ReminderTool {
    fn name(&self) -> String {
        "reminder".to_string()
    }

    fn description(&self) -> String {
        "Manage reminders. `add` schedules one (due = now + in_minutes), `list` shows them, \
         `complete`/`remove` take an id. kind `notify` pops a system notification; `message` \
         also sends a proactive message when it fires. Use get_time first when unsure."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["add", "list", "complete", "remove"] },
                "text": { "type": "string", "description": "What to remind about (add)." },
                "in_minutes": { "type": "number", "description": "Minutes from now (add)." },
                "repeat_minutes": { "type": "number", "description": "Repeat interval (add, optional)." },
                "kind": { "type": "string", "enum": ["notify", "message"], "description": "Default notify." },
                "id": { "type": "string", "description": "Reminder id (complete/remove)." }
            },
            "required": ["action"]
        })
    }

    fn approval_key(&self, _args: &Value) -> Option<harness::permissions::ApprovalKey> {
        None
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "list" => {
                let store = load();
                if store.is_empty() {
                    return Ok("No reminders.".to_string());
                }
                let now = now_secs() as u32;
                let mut out = String::new();
                for r in &store {
                    let when = if r.done {
                        "done".to_string()
                    } else if r.due <= now {
                        "due now".to_string()
                    } else {
                        format!("in {} min", (r.due - now) / 60)
                    };
                    let repeat = r
                        .repeat_secs
                        .map(|s| format!(", repeats every {} min", s / 60))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "- {} | {} | {:?}{repeat} | {}\n",
                        r.id, when, r.kind, r.text
                    ));
                }
                Ok(out)
            }
            "add" => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("'text' is required for add"))?;
                let minutes = args
                    .get("in_minutes")
                    .and_then(Value::as_f64)
                    .filter(|m| *m >= 0.0)
                    .ok_or_else(|| anyhow::anyhow!("'in_minutes' is required for add"))?;
                let kind = match args.get("kind").and_then(Value::as_str) {
                    Some("message") => ReminderKind::Message,
                    _ => ReminderKind::Notify,
                };
                let repeat_secs = args
                    .get("repeat_minutes")
                    .and_then(Value::as_f64)
                    .filter(|m| *m >= 1.0)
                    .map(|m| (m * 60.0) as u32);
                let mut store = load();
                let reminder = Reminder {
                    id: new_id(&store),
                    text: text.chars().take(500).collect(),
                    due: (now_secs() + (minutes * 60.0) as u64) as u32,
                    repeat_secs,
                    kind,
                    done: false,
                };
                store.push(reminder.clone());
                save(&store).map_err(anyhow::Error::msg)?;
                Ok(format!(
                    "Reminder {} set for {} minutes from now.",
                    reminder.id, minutes as u32
                ))
            }
            "complete" | "remove" => {
                let id = args
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|i| !i.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("'id' is required"))?;
                let mut store = load();
                let before = store.len();
                if action == "complete" {
                    if let Some(r) = store.iter_mut().find(|r| r.id == id) {
                        r.done = true;
                    }
                } else {
                    store.retain(|r| r.id != id);
                }
                if store.len() == before && action == "remove" {
                    bail!("No reminder with id '{id}'");
                }
                save(&store).map_err(anyhow::Error::msg)?;
                Ok(format!("Reminder {id} {action}d."))
            }
            _ => bail!("Unknown action '{action}'"),
        }
    }
}

/// List reminders for the UI.
#[tauri::command]
#[specta::specta]
pub async fn assistant_reminders_list() -> Result<Vec<Reminder>, String> {
    Ok(load())
}

/// Add a reminder from the UI (unix `due`).
#[tauri::command]
#[specta::specta]
pub async fn assistant_reminder_add(
    text: String,
    due: u32,
    repeat_secs: Option<u32>,
    kind: ReminderKind,
) -> Result<Reminder, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Reminder text is empty".to_string());
    }
    let mut store = load();
    let reminder = Reminder {
        id: new_id(&store),
        text: text.chars().take(500).collect(),
        due: due.max(now_secs() as u32),
        repeat_secs: repeat_secs.filter(|s| *s >= 60),
        kind,
        done: false,
    };
    store.push(reminder.clone());
    save(&store)?;
    Ok(reminder)
}

/// Mark a reminder done.
#[tauri::command]
#[specta::specta]
pub async fn assistant_reminder_complete(id: String) -> Result<(), String> {
    let mut store = load();
    if let Some(r) = store.iter_mut().find(|r| r.id == id) {
        r.done = true;
    }
    save(&store)
}

/// Delete a reminder.
#[tauri::command]
#[specta::specta]
pub async fn assistant_reminder_remove(id: String) -> Result<(), String> {
    let mut store = load();
    store.retain(|r| r.id != id);
    save(&store)
}

/// Background tick: fires due reminders every 30 s.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_reflect_try: Option<std::time::Instant> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            reflect_tick(&app, &mut last_reflect_try).await;
            let mut store = load();
            let now = now_secs() as u32;
            let mut due = Vec::new();
            let mut changed = false;
            for r in store.iter_mut().filter(|r| !r.done && r.due <= now) {
                due.push(r.clone());
                match r.repeat_secs.filter(|s| *s >= 60) {
                    Some(rep) => {
                        // Catch up over missed intervals in one step.
                        let missed = ((now - r.due) / rep + 1) as u64;
                        r.due += (missed * rep as u64) as u32;
                    }
                    None => r.done = true,
                }
                changed = true;
            }
            if changed {
                let _ = save(&store);
            }
            for reminder in due {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { fire(app, reminder).await });
            }
        }
    });
}

/// One opt-in reflection per day over assistant memory; retried hourly while
/// unavailable (server stopped, provider missing).
async fn reflect_tick(app: &AppHandle, last_try: &mut Option<std::time::Instant>) {
    let Some(state) = app.try_state::<crate::AppState>() else {
        return;
    };
    let config = state.config.lock().unwrap().clone();
    if !config.assistant.reflection {
        return;
    }
    let now = now_secs() as u32;
    if now.saturating_sub(config.assistant.reflection_last) < 24 * 3600 {
        return;
    }
    if last_try
        .map(|t| t.elapsed() < Duration::from_secs(3600))
        .unwrap_or(false)
    {
        return;
    }
    *last_try = Some(std::time::Instant::now());
    if state.assistant_paused.load(Ordering::SeqCst) {
        return;
    }
    match crate::chat::assistant_reflect(&state).await {
        Ok(text) => {
            {
                let mut cfg = state.config.lock().unwrap();
                cfg.assistant.reflection_last = now;
                let _ = cfg.save();
            }
            let _ = app.emit("assistant_event", json!({"type": "notice", "text": text}));
        }
        Err(e) => {
            // Empty memory and a stopped server are expected; stay quiet.
            if !e.contains("empty") && !e.contains("Start the server") {
                let _ = app.emit(
                    "assistant_event",
                    json!({"type": "notice", "text": format!("Reflection skipped: {e}")}),
                );
            }
        }
    }
}

async fn fire(app: AppHandle, reminder: Reminder) {
    let Some(state) = app.try_state::<crate::AppState>() else {
        return;
    };
    if state.assistant_paused.load(Ordering::SeqCst) {
        return;
    }
    let config = state.config.lock().unwrap().clone();
    if config.assistant.notify {
        alert(&app);
    }
    let _ = app.emit(
        "assistant_event",
        json!({"type": "reminder", "text": reminder.text}),
    );
    if reminder.kind == ReminderKind::Message && config.assistant.proactive {
        if let Err(e) = crate::chat::assistant_proactive(app.clone(), &state, reminder.text.clone()).await
        {
            let _ = app.emit(
                "assistant_event",
                json!({"type": "notice", "text": format!("Reminder turn failed: {e}")}),
            );
        }
    }
}

/// Cross-platform attention request: flash the overlay when it is up, else
/// the main window (taskbar/dock urgency, no OS-toast dependency).
fn alert(app: &AppHandle) {
    use tauri::{Manager, UserAttentionType};
    let overlay = app
        .get_webview_window("overlay")
        .filter(|w| w.is_visible().unwrap_or(false));
    let target = overlay.or_else(|| app.get_webview_window("main"));
    if let Some(win) = target {
        let _ = win.request_user_attention(Some(UserAttentionType::Critical));
    }
}
