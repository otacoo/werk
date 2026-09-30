//! Long-term memory: curated markdown files injected into the prompt.
//! Global file holds user facts; project files hold conventions.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::permissions::ApprovalKey;
use crate::tools::Tool;

/// Global memory path under the given base dir (None keeps project-only).
pub fn global_memory_path(base: Option<&Path>) -> Option<PathBuf> {
    base.map(|b| b.join("MEMORY.md"))
}

pub fn project_memory_path(root: &Path) -> PathBuf {
    root.join(".werk").join("MEMORY.md")
}

/// Injected prompt block; capped so a rambling file cannot eat the window.
pub const MEMORY_BLOCK_CAP: usize = 4_096;
/// Hard file size cap for writes through the tool.
pub const MEMORY_FILE_CAP: usize = 16_384;

/// UTC date as `YYYY-MM-DD` (civil-from-days; keeps the crate dependency-free).
pub fn today_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let z = secs.div_euclid(86_400) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 };
    format!("{year:04}-{month:02}-{day:02}")
}

/// One structured entry line: `- [date] topic: text` (newlines folded).
pub fn entry_line(topic: &str, text: &str) -> String {
    let topic = topic.trim();
    let topic = if topic.is_empty() { "note" } else { topic };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("- [{}] {topic}: {text}\n", today_iso())
}

pub fn load_block(global: Option<&Path>, root: &Path) -> String {
    let mut parts = Vec::new();
    let read_capped = |path: &Path| -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return None;
        }
        Some(if trimmed.chars().count() > MEMORY_BLOCK_CAP {
            let mut short: String = trimmed.chars().take(MEMORY_BLOCK_CAP).collect();
            short.push_str("\n[…]");
            short
        } else {
            trimmed
        })
    };
    if let Some(path) = global {
        if let Some(text) = read_capped(path) {
            parts.push(format!("User memory:\n{text}"));
        }
    }
    if let Some(text) = read_capped(&project_memory_path(root)) {
        parts.push(format!("Project memory:\n{text}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("\n\n{}", parts.join("\n\n"))
}

/// Curator tool; writes are approval-gated, `show` runs free.
pub struct RememberTool {
    project_path: PathBuf,
    global_path: Option<PathBuf>,
}

impl RememberTool {
    pub fn new(project_root: &Path, global_base: Option<&Path>) -> Self {
        Self {
            project_path: project_memory_path(project_root),
            global_path: global_memory_path(global_base),
        }
    }

    fn resolve(&self, scope: &str) -> Result<PathBuf> {
        match scope {
            "project" => Ok(self.project_path.clone()),
            "global" => self.global_path.clone().ok_or_else(|| {
                anyhow::anyhow!("No global memory file is configured")
            }),
            other => bail!("Unknown scope '{other}' (expected 'project' or 'global')"),
        }
    }
}

impl Tool for RememberTool {
    fn name(&self) -> String {
        "remember".to_string()
    }
    fn description(&self) -> String {
        "Curate long-term memory: note durable facts as dated entries, forget stale ones, show the file. Project scope unless the fact is about the user themselves.".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["note", "forget", "show"] },
                "text": { "type": "string" },
                "topic": { "type": "string", "description": "Short topic tag, e.g. 'build', 'preferences'." },
                "scope": { "type": "string", "enum": ["project", "global"] }
            },
            "required": ["action"]
        })
    }
    fn approval_key(&self, args: &Value) -> Option<ApprovalKey> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if action == "show" {
            return None;
        }
        Some(ApprovalKey { tool: self.name(), command: None })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("project");
        let path = self.resolve(scope)?;
        match action {
            "show" => Ok(std::fs::read_to_string(&path).unwrap_or_default()),
            "note" => {
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                if text.is_empty() {
                    bail!("'text' must not be empty for note");
                }
                let topic = args.get("topic").and_then(|v| v.as_str()).unwrap_or("");
                let entry = entry_line(topic, text);
                let current = std::fs::read_to_string(&path).unwrap_or_default();
                if current.len() + entry.len() + 1 > MEMORY_FILE_CAP {
                    bail!("Memory file too large (16 KiB max) — forget something first");
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = current;
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&entry);
                std::fs::write(&path, out)?;
                Ok(format!("Noted in {scope} memory."))
            }
            "forget" => {
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                if text.is_empty() {
                    bail!("'text' must not be empty for forget");
                }
                let current = std::fs::read_to_string(&path).unwrap_or_default();
                let kept: Vec<&str> = current
                    .lines()
                    .filter(|l| !l.to_lowercase().contains(&text.to_lowercase()))
                    .collect();
                if kept.len() == current.lines().count() {
                    return Ok("No matching lines found.".to_string());
                }
                let mut out = kept.join("\n");
                if !out.is_empty() {
                    out.push('\n');
                }
                std::fs::write(&path, out)?;
                Ok(format!("Forgot matching lines in {scope} memory."))
            }
            other => bail!("Unknown action '{other}' (expected note/forget/show)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(tag: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("werk-mem-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let global = root.join("global");
        std::fs::create_dir_all(&global).unwrap();
        (root, global)
    }

    #[test]
    fn note_show_forget_round_trip() {
        let (root, global) = roots("tool");
        let tool = RememberTool::new(&root, Some(&global));
        assert!(tool.approval_key(&json!({"action": "show"})).is_none());
        assert!(tool.approval_key(&json!({"action": "note"})).is_some());
        tool.execute(&json!({"action": "note", "text": "likes dark mode"}))
            .unwrap();
        let shown = tool.execute(&json!({"action": "show"})).unwrap();
        assert!(shown.contains("dark mode"));
        tool.execute(&json!({"action": "forget", "text": "dark mode"})).unwrap();
        assert!(!tool.execute(&json!({"action": "show"})).unwrap().contains("dark mode"));
        assert!(tool.execute(&json!({"action": "explode"})).is_err());
        assert!(tool.execute(&json!({"action": "note", "scope": "zone"})).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn notes_are_dated_and_topic_tagged() {
        let (root, global) = roots("entries");
        let tool = RememberTool::new(&root, Some(&global));
        tool.execute(&json!({"action": "note", "topic": "build", "text": "use\nninja"}))
            .unwrap();
        let shown = tool.execute(&json!({"action": "show"})).unwrap();
        let line = shown.lines().next().unwrap();
        assert!(line.starts_with("- [20"), "{line}");
        assert!(line.contains("] build: use ninja"), "{line}");
        // The default topic keeps the line structured too.
        let entry = entry_line("", "plain fact");
        assert!(entry.contains("] note: plain fact"), "{entry}");
        assert!(entry.starts_with("- [20"), "{entry}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_block_combines_scopes_with_cap() {
        let (root, global) = roots("block");
        std::fs::write(global.join("MEMORY.md"), "user likes tea").unwrap();
        std::fs::create_dir_all(root.join(".werk")).unwrap();
        std::fs::write(root.join(".werk").join("MEMORY.md"), "tabs not spaces").unwrap();
        let block = load_block(Some(&global.join("MEMORY.md")), &root);
        assert!(block.contains("tea") && block.contains("tabs"));
        assert!(load_block(None, &root.join("empty")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
