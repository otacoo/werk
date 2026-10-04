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
/// Hard file size cap for writes through the tool. Generous: prompts retrieve
/// only the entries that matter, so growth no longer floods the window.
pub const MEMORY_FILE_CAP: usize = 65_536;

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
    let text = fold_ws(text);
    format!("- [{}] {topic}: {text}\n", today_iso())
}

/// One structured memory entry: `- [YYYY-MM-DD] topic: text`.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryEntry {
    pub date: String,
    pub topic: String,
    pub text: String,
}

fn fold_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a memory file into entries. Lines that don't match the structured
/// shape become `note` entries with no date, so free-form files migrate on the
/// next render.
pub fn parse_entries(text: &str) -> Vec<MemoryEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rest = trimmed.trim_start_matches(['-', '*', '•', ' ']).trim();
        if rest.is_empty() {
            continue;
        }
        let (date, rest) = match rest.strip_prefix('[') {
            Some(inner) => match inner.split_once(']') {
                Some((d, r)) if !d.trim().is_empty() && d.trim().len() <= 24 => {
                    (d.trim().to_string(), r.trim())
                }
                _ => (String::new(), rest),
            },
            None => (String::new(), rest),
        };
        let (topic, text) = match rest.split_once(':') {
            Some((t, x))
                if !t.trim().is_empty() && t.trim().len() <= 24 && !t.trim().contains(' ') =>
            {
                (t.trim().to_string(), x.trim().to_string())
            }
            _ => ("note".to_string(), rest.to_string()),
        };
        out.push(MemoryEntry { date, topic, text: fold_ws(&text) });
    }
    out
}

/// Serialize entries back to the file format; empty dates become today and
/// empty topics become `note`.
pub fn render_entries(entries: &[MemoryEntry]) -> String {
    let today = today_iso();
    let mut out = String::new();
    for e in entries {
        let text = fold_ws(&e.text);
        if text.is_empty() {
            continue;
        }
        let date = if e.date.trim().is_empty() { today.clone() } else { e.date.trim().to_string() };
        let topic = if e.topic.trim().is_empty() { "note".to_string() } else { e.topic.trim().to_string() };
        out.push_str(&format!("- [{date}] {topic}: {text}\n"));
    }
    out
}

/// Keep the rendered file under `MEMORY_FILE_CAP`: the oldest entries are
/// merged into one `earlier` entry; as a last resort the head of the merged
/// text is dropped so the newest facts survive.
pub fn enforce_cap(entries: &mut Vec<MemoryEntry>) {
    while render_entries(entries).len() > MEMORY_FILE_CAP && entries.len() > 1 {
        let first = entries.remove(0);
        let second = &mut entries[0];
        second.text = if first.text.is_empty() {
            second.text.clone()
        } else {
            format!("{}; {}", first.text, second.text)
        };
        second.topic = "earlier".to_string();
        if !first.date.is_empty() {
            second.date = first.date;
        }
    }
    if render_entries(entries).len() > MEMORY_FILE_CAP {
        if let Some(only) = entries.first_mut() {
            let rendered = render_entries(std::slice::from_ref(only));
            let overhead = rendered.len().saturating_sub(fold_ws(&only.text).len());
            let budget = MEMORY_FILE_CAP.saturating_sub(overhead + "…".len());
            let chars: Vec<char> = only.text.chars().collect();
            let start = chars.len().saturating_sub(budget);
            only.text = format!("…{}", chars[start..].iter().collect::<String>());
        }
    }
}

/// Newest entries that always survive retrieval, whatever the query.
const RECENT_KEEP: usize = 5;

const STOPWORDS: [&str; 13] = [
    "the", "and", "for", "with", "that", "this", "what", "does", "how", "are", "was", "you",
    "your",
];

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase())
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .collect()
}

fn rendered_len(e: &MemoryEntry) -> usize {
    e.date.len() + e.topic.len() + e.text.len() + 6
}

/// Top-k retrieval: the newest `RECENT_KEEP` entries always survive, then the
/// best query matches fill the remaining budget (token overlap plus a recency
/// bonus). Returned in chronological order.
pub fn retrieve_entries(entries: &[MemoryEntry], query: &str, budget: usize) -> Vec<MemoryEntry> {
    if entries.is_empty() {
        return Vec::new();
    }
    let query_tokens = tokens(query);
    let total = entries.len();
    // Newest first, leaving at least half the budget for query matches.
    let mut picked: Vec<usize> = Vec::new();
    let mut used = 0usize;
    let recent_budget = (budget / 2).max(1);
    for i in (0..total).rev().take(RECENT_KEEP) {
        let len = rendered_len(&entries[i]);
        if !picked.is_empty() && used + len > recent_budget {
            break;
        }
        used += len;
        picked.push(i);
    }
    let mut scored: Vec<(usize, i64, i64)> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let hay = tokens(&format!("{} {} {}", e.topic, e.text, e.date));
            let overlap = query_tokens
                .iter()
                .filter(|q| hay.iter().any(|h| h == *q))
                .count() as i64;
            // Later lines are newer; a capped recency bonus never beats a match.
            (i, overlap, overlap * 30 + (i as i64 * 2).min(20))
        })
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.2));
    for (i, overlap, _) in scored {
        // Only real matches fill the rest; recency alone is not a reason.
        if overlap == 0 || picked.contains(&i) {
            continue;
        }
        let len = rendered_len(&entries[i]);
        if used + len > budget {
            continue;
        }
        used += len;
        picked.push(i);
    }
    picked.sort_unstable();
    picked.into_iter().map(|i| entries[i].clone()).collect()
}

/// Query-aware block for one memory file, under `label`.
pub fn load_file_query(path: &Path, label: &str, query: &str) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let entries = parse_entries(&text);
    if entries.is_empty() {
        return String::new();
    }
    let picked = retrieve_entries(&entries, query, MEMORY_BLOCK_CAP);
    if picked.is_empty() {
        return String::new();
    }
    format!("\n\n{label}:\n{}", render_entries(&picked).trim_end())
}

/// Query-aware `load_block`: relevant entries instead of the file head.
pub fn load_block_query(global: Option<&Path>, root: &Path, query: &str) -> String {
    let mut parts = Vec::new();
    let read = |path: &Path| -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        let entries = parse_entries(&text);
        if entries.is_empty() {
            return None;
        }
        let picked = retrieve_entries(&entries, query, MEMORY_BLOCK_CAP);
        if picked.is_empty() {
            return None;
        }
        Some(render_entries(&picked).trim_end().to_string())
    };
    if let Some(path) = global {
        if let Some(text) = read(path) {
            parts.push(format!("User memory:\n{text}"));
        }
    }
    if let Some(text) = read(&project_memory_path(root)) {
        parts.push(format!("Project memory:\n{text}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("\n\n{}", parts.join("\n\n"))
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
    /// Roleplay: a single per-character file; the scope argument is ignored.
    single_scope: bool,
}

impl RememberTool {
    pub fn new(project_root: &Path, global_base: Option<&Path>) -> Self {
        Self {
            project_path: project_memory_path(project_root),
            global_path: global_memory_path(global_base),
            single_scope: false,
        }
    }

    /// Single-file variant for roleplay: one memory per character card.
    pub fn for_file(path: PathBuf) -> Self {
        Self {
            project_path: path,
            global_path: None,
            single_scope: true,
        }
    }

    fn resolve(&self, scope: &str) -> Result<PathBuf> {
        if self.single_scope {
            return Ok(self.project_path.clone());
        }
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
        if self.single_scope {
            "Curate long-term memory for this story: note durable facts about the user, the character, and past events as dated entries, forget stale ones, show the file.".to_string()
        } else {
            "Curate long-term memory: note durable facts as dated entries, forget stale ones, show the file. Project scope unless the fact is about the user themselves.".to_string()
        }
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
        // Roleplay curation runs free: there is no approval UI in Talk.
        if action == "show" || self.single_scope {
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
                let mut entries = parse_entries(&std::fs::read_to_string(&path).unwrap_or_default());
                entries.push(MemoryEntry {
                    date: today_iso(),
                    topic: topic.trim().to_string(),
                    text: text.to_string(),
                });
                // Free-form lines migrate here; overflow folds the oldest.
                enforce_cap(&mut entries);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, render_entries(&entries))?;
                Ok(if self.single_scope {
                    "Noted in memory.".to_string()
                } else {
                    format!("Noted in {scope} memory.")
                })
            }
            "forget" => {
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                if text.is_empty() {
                    bail!("'text' must not be empty for forget");
                }
                let entries = parse_entries(&std::fs::read_to_string(&path).unwrap_or_default());
                let total = entries.len();
                let needle = text.to_lowercase();
                let kept: Vec<MemoryEntry> = entries
                    .into_iter()
                    .filter(|e| {
                        !e.text.to_lowercase().contains(&needle)
                            && !e.topic.to_lowercase().contains(&needle)
                    })
                    .collect();
                if kept.len() == total {
                    return Ok("No matching entries found.".to_string());
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, render_entries(&kept))?;
                Ok(if self.single_scope {
                    "Forgot matching entries.".to_string()
                } else {
                    format!("Forgot matching entries in {scope} memory.")
                })
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

    #[test]
    fn free_form_lines_migrate_to_entries() {
        let entries = parse_entries(
            "user likes tea\n- [2026-01-02] build: use ninja\n\n* preferences: dark mode\n",
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0],
            MemoryEntry {
                date: String::new(),
                topic: "note".into(),
                text: "user likes tea".into()
            }
        );
        assert_eq!(entries[1].topic, "build");
        assert_eq!(entries[1].date, "2026-01-02");
        assert_eq!(entries[2].topic, "preferences");
        let out = render_entries(&entries);
        assert!(out.contains("] note: user likes tea"), "{out}");
        assert!(out.contains("[2026-01-02] build: use ninja"), "{out}");
        assert!(out.contains("preferences: dark mode"), "{out}");
        assert_eq!(parse_entries(&out).len(), 3);
    }

    #[test]
    fn retrieval_keeps_recent_and_relevant_entries() {
        let mut entries = vec![MemoryEntry {
            date: "2026-01-01".into(),
            topic: "prefs".into(),
            text: "alpha unique thing".into(),
        }];
        for i in 0..40 {
            entries.push(MemoryEntry {
                date: "2026-01-01".into(),
                topic: "log".into(),
                text: format!("entry {i} {}", "filler ".repeat(20)),
            });
        }
        // The oldest entry matches; the newest few always survive too.
        let picked = retrieve_entries(&entries, "alpha thing", 800);
        assert!(
            picked.iter().any(|e| e.text.contains("alpha unique")),
            "{} entries",
            picked.len()
        );
        assert!(picked.iter().any(|e| e.text.contains("entry 39")));
        assert!(picked.len() < 15, "{}", picked.len());
        // Nothing matches: the newest entries still come back.
        let picked = retrieve_entries(&entries, "zzz", 400);
        assert!(!picked.is_empty());
        assert!(picked.iter().all(|e| e.text.starts_with("entry")));
    }

    #[test]
    fn query_block_prefers_matching_entries() {
        let (root, global) = roots("topk");
        let path = global.join("MEMORY.md");
        let mut text = String::from("- [2026-01-01] prefs: alpha unique thing\n");
        for i in 0..10 {
            text.push_str(&format!(
                "- [2026-01-{:02}] log: filler {i} {}\n",
                i + 2,
                "pad ".repeat(30)
            ));
        }
        std::fs::write(&path, text).unwrap();
        let block = load_file_query(&path, "User memory", "alpha thing");
        assert!(block.contains("alpha unique thing"), "{block}");
        assert!(block.contains("User memory:"), "{block}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn overflow_folds_the_oldest_entries() {
        let mut entries: Vec<MemoryEntry> = (0..300)
            .map(|i| MemoryEntry {
                date: "2026-01-01".into(),
                topic: "log".into(),
                text: format!("entry {i} {}", "x".repeat(300)),
            })
            .collect();
        entries.push(MemoryEntry {
            date: today_iso(),
            topic: "new".into(),
            text: "the newest fact".into(),
        });
        enforce_cap(&mut entries);
        let out = render_entries(&entries);
        assert!(out.len() <= MEMORY_FILE_CAP, "{}", out.len());
        assert!(out.contains("the newest fact"), "{out}");
        assert!(out.contains("earlier"), "{out}");
    }
}
