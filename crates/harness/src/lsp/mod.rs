//! Lean language-server integration: one server per (spec, project root),
//! spawned on first use, killed on shutdown. Diagnostics ride back into the
//! write/edit tool result; `tool` exposes the read-only query tool.

pub mod client;
pub mod servers;
pub mod tool;
pub mod uri;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use client::Client;
use servers::ServerSpec;

/// How long a write waits for fresh diagnostics before moving on.
const DIAG_TIMEOUT: Duration = Duration::from_secs(3);
const BACKOFF_MIN: Duration = Duration::from_secs(10);
const BACKOFF_MAX: Duration = Duration::from_secs(300);
const MAX_DIAG_LINES: usize = 8;

pub struct LspManager {
    inner: Arc<Inner>,
}

struct Inner {
    /// Configured servers; replaced whenever settings change.
    specs: RwLock<Vec<Arc<ServerSpec>>>,
    slots: Mutex<HashMap<(String, PathBuf), Slot>>,
    backoff: Mutex<HashMap<String, Duration>>,
}

enum Slot {
    Ready(Arc<Client>),
    Cooling(Instant),
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                specs: RwLock::new(Vec::new()),
                slots: Mutex::new(HashMap::new()),
                backoff: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Replace the server set; running servers are killed since extension
    /// ownership may have changed.
    pub fn set_specs(&self, specs: Vec<ServerSpec>) {
        *self.inner.specs.write().unwrap() = specs.into_iter().map(Arc::new).collect();
        self.shutdown_all();
        self.inner.backoff.lock().unwrap().clear();
    }

    /// Running server for `file`, spawning and initializing on first use.
    /// Failures park the server in a cooldown instead of retrying per write.
    pub(crate) fn client(&self, file: &Path, root: &Path) -> Option<(Arc<Client>, Arc<ServerSpec>)> {
        let specs = self.inner.specs.read().unwrap().clone();
        let spec = servers::spec_for(&specs, file)?.clone();
        let key = (spec.name.clone(), servers::root_for(&spec, file, root));
        let mut slots = self.inner.slots.lock().unwrap();
        match slots.get(&key) {
            Some(Slot::Ready(client)) => return Some((client.clone(), spec)),
            Some(Slot::Cooling(until)) if *until > Instant::now() => return None,
            _ => {}
        }
        // Spawn under the lock: one agent run at a time, no duplicate servers.
        let program = spec.command.first()?;
        let spawned = servers::resolve_program(program).map(|bin| Client::spawn(&spec, &bin, &key.1));
        match spawned {
            Some(Ok(client)) => {
                self.inner
                    .backoff
                    .lock()
                    .unwrap()
                    .insert(spec.name.clone(), BACKOFF_MIN);
                slots.insert(key, Slot::Ready(client.clone()));
                Some((client, spec))
            }
            _ => {
                let mut backoff = self.inner.backoff.lock().unwrap();
                let next =
                    (backoff.get(&spec.name).copied().unwrap_or(BACKOFF_MIN) * 2).min(BACKOFF_MAX);
                backoff.insert(spec.name.clone(), next);
                slots.insert(key, Slot::Cooling(Instant::now() + next));
                None
            }
        }
    }

    /// `<diagnostics>` block for a just-written file; ERROR severity only.
    /// `None` when no server is available, nothing arrived in time, or clean.
    pub fn diagnostics_on_write(&self, file: &Path, root: &Path, text: &str) -> Option<String> {
        let (client, spec) = self.client(file, root)?;
        let uri = uri::path_to_uri(file);
        let language = servers::language_id(&spec, file);
        client.open_or_change(&uri, language, text);
        let items = client.wait_diagnostics(&uri, DIAG_TIMEOUT)?;
        format_diagnostics(root, file, &items)
    }

    /// Kill every running server (LSP disabled or app exit).
    pub fn shutdown_all(&self) {
        let mut slots = self.inner.slots.lock().unwrap();
        for slot in slots.values() {
            if let Slot::Ready(client) = slot {
                client.kill();
            }
        }
        slots.clear();
    }
}

fn format_diagnostics(root: &Path, file: &Path, items: &[serde_json::Value]) -> Option<String> {
    let mut lines = Vec::new();
    for d in items {
        if d.get("severity").and_then(serde_json::Value::as_u64) != Some(1) {
            continue;
        }
        let start = d.pointer("/range/start");
        let line = start
            .and_then(|s| s.get("line"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            + 1;
        let col = start
            .and_then(|s| s.get("character"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            + 1;
        let mut msg: String = d
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(160)
            .collect();
        if let Some(code) = d.get("code") {
            let code = match code {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            if !code.is_empty() {
                msg.push_str(&format!(" [{code}]"));
            }
        }
        lines.push(format!("  {line}:{col}: {msg}"));
        if lines.len() >= MAX_DIAG_LINES {
            break;
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "<diagnostics file=\"{}\">\n{}\n</diagnostics>",
        rel_display(root, file),
        lines.join("\n")
    ))
}

pub(crate) fn rel_display(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diagnostics_keep_only_errors_and_cap_lines() {
        let root = Path::new("/proj");
        let file = Path::new("/proj/src/main.rs");
        let mut items = vec![json!({
            "severity": 2,
            "message": "unused variable",
            "range": {"start": {"line": 0, "character": 0}}
        })];
        assert!(format_diagnostics(root, file, &items).is_none());

        items.push(json!({
            "severity": 1,
            "message": "mismatched types\nmore detail here",
            "code": "E0308",
            "range": {"start": {"line": 41, "character": 4}}
        }));
        items.push(json!({
            "severity": 1,
            "message": "expected `;`",
            "range": {"start": {"line": 7, "character": 1}}
        }));
        let block = format_diagnostics(root, file, &items).unwrap();
        assert!(block.starts_with("<diagnostics file=\"src/main.rs\">"));
        assert!(block.contains("42:5: mismatched types [E0308]"));
        assert!(block.contains("8:2: expected `;`"));
        assert!(!block.contains("unused variable"));
        assert!(block.ends_with("</diagnostics>"));

        let filler: Vec<_> = (0..20)
            .map(|i| {
                json!({
                    "severity": 1,
                    "message": format!("err {i}"),
                    "range": {"start": {"line": i, "character": 0}}
                })
            })
            .collect();
        let block = format_diagnostics(root, file, &filler).unwrap();
        assert_eq!(block.matches("\n  ").count(), MAX_DIAG_LINES);
    }

    #[test]
    fn files_without_a_spec_have_no_server() {
        let manager = LspManager::new();
        let dir = std::env::temp_dir().join(format!("werk-lsp-nospec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("probe.thisisntreal");
        assert!(manager.client(&file, &dir).is_none());
        // With no configured servers, even known extensions have none.
        assert!(manager.client(&dir.join("probe.rs"), &dir).is_none());
        manager.set_specs(vec![ServerSpec::build(
            "probe",
            "probe-lsp-not-installed",
            &["thisisntreal".to_string()],
            None,
            &[],
        )
        .unwrap()]);
        // Spec exists but the program is missing: cooldown, still None.
        assert!(manager.client(&file, &dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
