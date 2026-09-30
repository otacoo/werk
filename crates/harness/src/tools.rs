//! Sandboxed file/shell tools plus schema-only orchestrator tools.
//! Every `execute` is synchronous; run it on a blocking thread.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::permissions::{ApprovalKey, Decision, PermissionEngine};
use crate::sandbox::PathJail;

pub const READ_CAP: usize = 100_000;
pub const FIND_CAP: usize = 100;
pub const SEARCH_CAP: usize = 50;
pub const EXEC_CAP: usize = 50_000;
pub const EXEC_TIMEOUT_SECS: u64 = 120;

pub trait Tool: Send + Sync {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn parameters(&self) -> Value;
    /// None runs free; Some gates through the permission engine.
    fn approval_key(&self, args: &Value) -> Option<ApprovalKey>;
    fn execute(&self, args: &Value) -> Result<String>;
}

fn str_arg(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("'{key}' must be a non-empty string"))
}

// ── read_file ─────────────────────────────────────────────────────────────

pub struct ReadFileTool {
    jail: Arc<PathJail>,
}

impl Tool for ReadFileTool {
    fn name(&self) -> String {
        "read_file".to_string()
    }
    fn description(&self) -> String {
        "Read a text file relative to the project directory.".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path relative to the project" }
            },
            "required": ["path"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let path = self.jail.check_read(&str_arg(args, "path")?)?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        if text.chars().count() > READ_CAP {
            let n = text.chars().count();
            let head: String = text.chars().take(READ_CAP).collect();
            return Ok(format!("{head}\n[…truncated — {} more characters]", n - READ_CAP));
        }
        Ok(text)
    }
}

// ── write_file ────────────────────────────────────────────────────────────

pub struct WriteFileTool {
    jail: Arc<PathJail>,
    lsp: Option<Arc<crate::lsp::LspManager>>,
}

impl Tool for WriteFileTool {
    fn name(&self) -> String {
        "write_file".to_string()
    }
    fn description(&self) -> String {
        "Create or overwrite a file relative to the project directory.".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        Some(ApprovalKey { tool: self.name(), command: None })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let path = self.jail.check_write(&str_arg(args, "path")?)?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        let mut out = format!("Wrote {} bytes to {}", content.len(), path.display());
        if let Some(lsp) = &self.lsp {
            if let Some(block) = lsp.diagnostics_on_write(&path, self.jail.root(), content) {
                out.push('\n');
                out.push_str(&block);
            }
        }
        Ok(out)
    }
}

// ── edit_file ─────────────────────────────────────────────────────────────

pub struct EditFileTool {
    jail: Arc<PathJail>,
    lsp: Option<Arc<crate::lsp::LspManager>>,
}

impl EditFileTool {
    fn replace_once(text: &str, search: &str, replace: &str) -> Result<String> {
        let hits = text.matches(search).count();
        if hits != 1 {
            bail!("Search text matches {hits} times (must match exactly once)");
        }
        Ok(text.replacen(search, replace, 1))
    }
}

impl Tool for EditFileTool {
    fn name(&self) -> String {
        "edit_file".to_string()
    }
    fn description(&self) -> String {
        "Exact search/replace edit; the search text must occur exactly once.".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "search": { "type": "string" },
                "replace": { "type": "string" }
            },
            "required": ["path", "search", "replace"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        Some(ApprovalKey { tool: self.name(), command: None })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let path = self.jail.check_write(&str_arg(args, "path")?)?;
        let search = str_arg(args, "search")?;
        let replace = args.get("replace").and_then(|v| v.as_str()).unwrap_or_default();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        // Three passes: exact, CRLF-flipped, trailing-whitespace healed.
        let (new, how) = if let Ok(n) = Self::replace_once(&text, &search, replace) {
            (n, "")
        } else {
            let flipped = flip_line_endings(&search);
            if flipped != search {
                if let Ok(n) = Self::replace_once(&text, &flipped, replace) {
                    (n, " (after normalizing line endings)")
                } else {
                    heal_trailing_whitespace(&text, &search, replace)?
                }
            } else {
                heal_trailing_whitespace(&text, &search, replace)?
            }
        };
        std::fs::write(&path, &new)?;
        let removed = search.lines().count();
        let added = replace.lines().count();
        let mut out = format!(
            "Edited {} (+{added} -{removed}) (1 match replaced{how})",
            path.display()
        );
        if let Some(lsp) = &self.lsp {
            if let Some(block) = lsp.diagnostics_on_write(&path, self.jail.root(), &new) {
                out.push('\n');
                out.push_str(&block);
            }
        }
        Ok(out)
    }
}

fn flip_line_endings(s: &str) -> String {
    if s.contains("\r\n") {
        s.replace("\r\n", "\n")
    } else {
        s.replace('\n', "\r\n")
    }
}

fn heal_trailing_whitespace(text: &str, search: &str, replace: &str) -> Result<(String, &'static str)> {
    let norm = |s: &str| {
        s.lines()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let normed_search = norm(search);
    let mut matches = Vec::new();
    // Slide a window of normed-search height over normed text lines.
    let text_lines: Vec<&str> = text.lines().collect();
    let height = normed_search.lines().count().max(1);
    let max_start = text_lines.len().saturating_sub(height);
    for start in 0..=max_start {
        let end = (start + height).min(text_lines.len());
        if end <= start {
            continue;
        }
        let window = text_lines[start..end]
            .iter()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        if window == normed_search {
            matches.push(start);
        }
    }
    if matches.len() != 1 {
        bail!(
            "Search text matches {} times (must match exactly once)",
            text.matches(&search).count()
        );
    }
    let start = matches[0];
    let end = (start + height).min(text_lines.len());
    let mut out = text_lines[..start].to_vec();
    out.extend(replace.lines());
    out.extend_from_slice(&text_lines[end..]);
    // Preserve a trailing newline when the original had one.
    let mut joined = out.join("\n");
    if text.ends_with('\n') && !joined.ends_with('\n') {
        joined.push('\n');
    }
    Ok((joined, " (ignoring trailing whitespace)"))
}

// ── find_files ────────────────────────────────────────────────────────────

fn ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | ".venv" | "__pycache__"
    )
}

pub struct FindFilesTool {
    jail: Arc<PathJail>,
}

impl Tool for FindFilesTool {
    fn name(&self) -> String {
        "find_files".to_string()
    }
    fn description(&self) -> String {
        "Glob file search relative to the project (skips VCS/build dirs).".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob like src/**/*.rs" }
            },
            "required": ["pattern"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let pattern = str_arg(args, "pattern")?;
        let mut hits = Vec::new();
        for entry in walkdir::WalkDir::new(self.jail.root())
            .into_iter()
            .filter_entry(|e| {
                e.file_name().to_str().map(|n| !ignored_dir(n)).unwrap_or(true)
            })
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(self.jail.root())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if glob::Pattern::new(&pattern)
                .map(|pat| pat.matches(&rel))
                .unwrap_or(false)
            {
                hits.push(rel);
                if hits.len() >= FIND_CAP {
                    break;
                }
            }
        }
        if hits.is_empty() {
            return Ok(format!("No files match '{pattern}'"));
        }
        Ok(hits.join("\n"))
    }
}

// ── search_content ────────────────────────────────────────────────────────

pub struct SearchContentTool {
    jail: Arc<PathJail>,
}

impl Tool for SearchContentTool {
    fn name(&self) -> String {
        "search_content".to_string()
    }
    fn description(&self) -> String {
        "Regex content search over project text files.".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Regex to search for" },
                "pattern": { "type": "string", "description": "Optional file glob filter" }
            },
            "required": ["query"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let query = str_arg(args, "query")?;
        let re = regex::Regex::new(&query).map_err(|e| anyhow::anyhow!("Invalid regex: {e}"))?;
        let file_filter = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let glob_pat = if file_filter.is_empty() {
            None
        } else {
            glob::Pattern::new(file_filter).ok()
        };
        let mut hits = Vec::new();
        for entry in walkdir::WalkDir::new(self.jail.root())
            .into_iter()
            .filter_entry(|e| {
                e.file_name().to_str().map(|n| !ignored_dir(n)).unwrap_or(true)
            })
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.metadata().map(|m| m.len() > 2_000_000).unwrap_or(true) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(self.jail.root())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if let Some(pat) = &glob_pat {
                if !pat.matches(&rel) {
                    continue;
                }
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if text.as_bytes().contains(&0) {
                continue;
            }
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{}:{}: {}", rel, i + 1, line.trim()));
                    if hits.len() >= SEARCH_CAP {
                        break;
                    }
                }
            }
            if hits.len() >= SEARCH_CAP {
                break;
            }
        }
        if hits.is_empty() {
            return Ok(format!("No matches for '{query}'"));
        }
        Ok(hits.join("\n"))
    }
}

// ── exec ──────────────────────────────────────────────────────────────────

/// Read-only shell heads run free; everything else needs approval.
fn command_is_readonly(command: &str) -> bool {
    let head = command.trim_start().to_lowercase();
    let first = head.split_whitespace().next().unwrap_or("");
    // PowerShell verbs and POSIX tools that only inspect.
    const READONLY: &[&str] = &[
        "get-childitem", "get-content", "select-string", "get-location", "get-date",
        "ls", "dir", "cat", "pwd", "echo", "printf", "git",
    ];
    if first == "git" {
        // Only plumbing/status reads; pushes, commits, checkouts need approval.
        return matches!(
            head.split_whitespace().nth(1).unwrap_or(""),
            "status" | "log" | "diff" | "show" | "branch" | "ls-files" | "rev-parse" | "remote"
        );
    }
    READONLY.contains(&first)
}

/// Warn when shell syntax mismatches the OS (PowerShell vs sh).
fn shell_mismatch_hint(command: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        for pat in ["ls -la", "grep ", "cat ", "&&", "/dev/null"] {
            if command.contains(pat) {
                return Some(format!(
                    "This is Windows PowerShell, not sh: avoid '{pat}' (use Get-ChildItem, Select-String, ';')."
                ));
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        for pat in ["Get-ChildItem", "Get-Content", "Select-String"] {
            if command.contains(pat) {
                return Some(format!(
                    "This is POSIX sh, not PowerShell: avoid '{pat}' (use ls, cat, grep)."
                ));
            }
        }
    }
    None
}

/// Kill a timed-out command and anything it spawned (children hold the pipes).
fn kill_tree(child: &mut std::process::Child) {
    let pid = child.id();
    #[cfg(target_os = "windows")]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x08000000)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub struct ExecTool {
    jail: Arc<PathJail>,
}

impl Tool for ExecTool {
    fn name(&self) -> String {
        "exec".to_string()
    }
    fn description(&self) -> String {
        "Run a shell command in the project directory. Read-only commands run free.".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            },
            "required": ["command"]
        })
    }
    fn approval_key(&self, args: &Value) -> Option<ApprovalKey> {
        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        if command_is_readonly(command) {
            return None;
        }
        let head = command.split_whitespace().next().unwrap_or("").to_lowercase();
        Some(ApprovalKey { tool: self.name(), command: Some(head) })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let command = str_arg(args, "command")?;
        if let Some(hint) = shell_mismatch_hint(&command) {
            bail!("Wrong-shell syntax for this OS: {hint}. Rewrite the command and try again.");
        }
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = std::process::Command::new("powershell");
            c.args(["-NoLogo", "-NoProfile", "-Command", &command]);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(&command);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                // Own process group so the timeout can kill the whole tree.
                c.process_group(0);
            }
            c
        };
        cmd.current_dir(self.jail.root())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(target_os = "windows")]
        {
            #[allow(unused_imports)]
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn command: {command}"))?;
        let stdout = child.stdout.take().context("no stdout pipe")?;
        let stderr = child.stderr.take().context("no stderr pipe")?;
        // Readers report over a channel instead of being joined: a grandchild
        // that inherited the pipes keeps them open past the child's exit, and
        // a blocking join would hang the tool (and the whole agent run).
        let (tx, rx) = std::sync::mpsc::channel::<(bool, Vec<u8>)>();
        let tx_out = tx.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = std::io::BufReader::new(stdout).read_to_end(&mut buf);
            let _ = tx_out.send((true, buf));
        });
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = std::io::BufReader::new(stderr).read_to_end(&mut buf);
            let _ = tx.send((false, buf));
        });
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(EXEC_TIMEOUT_SECS);
        let mut status: Option<std::process::ExitStatus> = None;
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(s)) => {
                    status = Some(s);
                    break;
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        kill_tree(&mut child);
                        timed_out = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => bail!("Failed to wait for command: {e}"),
            }
        }
        let mut out_buf = Vec::new();
        let mut err_buf = Vec::new();
        let (mut have_out, mut have_err) = (false, false);
        let collect_until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !(have_out && have_err) {
            let left = collect_until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(left) {
                Ok((true, buf)) => {
                    out_buf = buf;
                    have_out = true;
                }
                Ok((false, buf)) => {
                    err_buf = buf;
                    have_err = true;
                }
                Err(_) => break,
            }
        }
        let mut text = String::from_utf8_lossy(&out_buf).to_string();
        let err_text = String::from_utf8_lossy(&err_buf).trim().to_string();
        if !err_text.is_empty() {
            text.push_str("\n[stderr]\n");
            text.push_str(&err_text);
        }
        if text.chars().count() > EXEC_CAP {
            let n = text.chars().count();
            text = text.chars().take(EXEC_CAP).collect();
            text.push_str(&format!("\n[truncated — {} more characters]", n - EXEC_CAP));
        }
        if timed_out {
            text.push_str(&format!("\n[command timed out after {EXEC_TIMEOUT_SECS}s and was killed]"));
        } else if let Some(s) = &status {
            if !s.success() {
                text.push_str(&format!("\n[exit code {}]", s.code().unwrap_or(-1)));
            }
        }
        Ok(text)
    }
}

// ── spawn_subagent (schema-only; intercepted by the loop) ─────────────────

pub struct SpawnSubagentTool;

impl Tool for SpawnSubagentTool {
    fn name(&self) -> String {
        "spawn_subagent".to_string()
    }
    fn description(&self) -> String {
        "Delegate a focused task to an ephemeral specialist subagent ('coder' implements, 'researcher' investigates). Only its final report returns.".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string", "description": "Self-contained task for the subagent" },
                "agent_type": { "type": "string", "enum": ["coder", "researcher"] }
            },
            "required": ["goal", "agent_type"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }
    fn execute(&self, _args: &Value) -> Result<String> {
        bail!("spawn_subagent must be run by the orchestrator loop")
    }
}

// ── ask_user (schema-only; intercepted by the loop) ───────────────────────

pub struct AskUserTool;

impl Tool for AskUserTool {
    fn name(&self) -> String {
        "ask_user".to_string()
    }
    fn description(&self) -> String {
        "Ask the user one multiple-choice question (2-4 options) when their input is genuinely needed. Never use it for facts you can look up.".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 6 }
            },
            "required": ["question", "options"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }
    fn execute(&self, _args: &Value) -> Result<String> {
        bail!("ask_user must be run by the orchestrator loop")
    }
}

// ── Argument repair ─────────────────────────────────────────────────────────

/// Repairs applied to a tool call's args; reported for observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgRepair {
    NullStripped,
    JsonStringToArray,
    BareStringToArray,
    MdLinkUnwrapped,
}

const PATH_LIKE_FIELDS: &[&str] = &["path", "dir", "file", "filename", "pattern"];

/// Validate-then-repair: schema-matching args pass through untouched.
/// Otherwise fix recoverable shapes — null-valued optionals, JSON or bare
/// strings at array fields, markdown links in path fields.
pub fn repair_tool_args(schema: &Value, args: &Value) -> (Value, Vec<ArgRepair>) {
    let mut repairs = Vec::new();
    let Value::Object(map) = args else {
        return (args.clone(), repairs);
    };
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return (args.clone(), repairs);
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let mut out = map.clone();
    for (key, value) in map {
        if value.is_null() && !required.iter().any(|r| r == key) {
            out.remove(key);
            repairs.push(ArgRepair::NullStripped);
            continue;
        }
        let field_type = props
            .get(key)
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if field_type == "array" {
            if let Some(s) = value.as_str() {
                let trimmed = s.trim();
                if trimmed.starts_with('[') {
                    if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(trimmed) {
                        out.insert(key.clone(), Value::Array(arr));
                        repairs.push(ArgRepair::JsonStringToArray);
                        continue;
                    }
                }
                out.insert(key.clone(), Value::Array(vec![Value::String(s.to_string())]));
                repairs.push(ArgRepair::BareStringToArray);
                continue;
            }
        }
        if PATH_LIKE_FIELDS.contains(&key.as_str()) {
            if let Some(s) = value.as_str() {
                if let Some(unwrapped) = unwrap_md_link(s) {
                    out.insert(key.clone(), Value::String(unwrapped));
                    repairs.push(ArgRepair::MdLinkUnwrapped);
                }
            }
        }
    }
    (Value::Object(out), repairs)
}

/// Unwrap degenerate auto-links (`[x](http://x)`, `[a/b](…/a/b)`).
/// Real links (text differs from target) pass through.
fn unwrap_md_link(s: &str) -> Option<String> {
    let t = s.trim();
    let inner = t.strip_prefix('[')?;
    let (text, rest) = inner.split_once("](")?;
    let url = rest.strip_suffix(')')?;
    if text.is_empty() || text.contains([' ', '\n', '\t']) || url.contains([' ', '\n', '\t']) {
        return None;
    }
    let no_protocol = url.split("://").last().unwrap_or(url);
    if text == no_protocol || url.ends_with(&format!("/{text}")) {
        return Some(text.to_string());
    }
    None
}

/// True for commands that change no state and return no new information.
/// Narrow by design: pipes, redirects, substitution, globs, and background
/// jobs all disqualify, so a miss just runs the command.
pub fn is_inert_shell_command(cmd: &str) -> bool {
    let mut any_segment = false;
    for chunk in cmd.split(['\n', ';']) {
        for part in chunk.split("&&").flat_map(|s| s.split("||")) {
            let seg = part.trim();
            if seg.is_empty() {
                continue;
            }
            any_segment = true;
            let head = seg.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            let rest = seg.split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
            let plain_output = (head == "echo" || head == "printf")
                && !rest.contains([
                    '|', '>', '<', '$', '`', '(', ')', '{', '}', '[', ']', '*', '?', '~', '#',
                    '&',
                ]);
            let null_builtin = matches!(head.as_str(), "true" | "false" | ":") && rest.is_empty();
            if !(plain_output || null_builtin) {
                return false;
            }
        }
    }
    any_segment
}

// ── Registry ────────────────────────────────────────────────────────────────

/// Static metadata row for the Tools settings surface.
pub struct BuiltinToolInfo {
    pub name: &'static str,
    pub summary: &'static str,
    /// "auto" | "approval" | "conditional".
    pub approval: &'static str,
    pub note: &'static str,
}

/// Registry of tools; subagents get an allowlisted subset (no recursion).
#[derive(Clone)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    jail: Option<Arc<PathJail>>,
}

impl ToolRegistry {
    pub fn project_tools(jail: Arc<PathJail>) -> Self {
        Self {
            tools: vec![
                Arc::new(ReadFileTool { jail: jail.clone() }),
                Arc::new(WriteFileTool { jail: jail.clone(), lsp: None }),
                Arc::new(EditFileTool { jail: jail.clone(), lsp: None }),
                Arc::new(FindFilesTool { jail: jail.clone() }),
                Arc::new(SearchContentTool { jail: jail.clone() }),
                Arc::new(ExecTool { jail: jail.clone() }),
                Arc::new(SpawnSubagentTool),
                Arc::new(AskUserTool),
            ],
            jail: Some(jail),
        }
    }

    /// Rebuild the file-writing tools so they report language-server
    /// diagnostics, and add the `lsp` query tool.
    pub fn with_lsp(mut self, lsp: Arc<crate::lsp::LspManager>) -> Self {
        let Some(jail) = self.jail.clone() else {
            return self;
        };
        self.tools = self
            .tools
            .into_iter()
            .map(|tool| -> Arc<dyn Tool> {
                match tool.name().as_str() {
                    "write_file" => Arc::new(WriteFileTool {
                        jail: jail.clone(),
                        lsp: Some(lsp.clone()),
                    }),
                    "edit_file" => Arc::new(EditFileTool {
                        jail: jail.clone(),
                        lsp: Some(lsp.clone()),
                    }),
                    _ => tool,
                }
            })
            .collect();
        self.add(Arc::new(crate::lsp::tool::LspTool::new(jail, lsp)))
    }

    pub fn add(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn without(self, names: &[&str]) -> Self {
        Self {
            tools: self
                .tools
                .into_iter()
                .filter(|t| !names.contains(&t.name().as_str()))
                .collect(),
            jail: self.jail,
        }
    }

    pub fn only(self, names: &[&str]) -> Self {
        Self {
            tools: self
                .tools
                .into_iter()
                .filter(|t| names.contains(&t.name().as_str()))
                .collect(),
            jail: self.jail,
        }
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| &**t)
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    pub fn tool_schemas(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.parameters(),
                    }
                })
            })
            .collect()
    }

    pub fn check_permissions(
        &self,
        name: &str,
        args: &Value,
        engine: &mut PermissionEngine,
        project: Option<&str>,
    ) -> bool {
        match self.get(name) {
            None => false,
            Some(tool) => match tool.approval_key(args) {
                None => true,
                Some(key) => engine.check(&key, project) == Decision::Allowed,
            },
        }
    }

    /// Run on the blocking pool; file/process work must not stall the runtime.
    pub fn spawn_execute(
        self: &Arc<Self>,
        name: String,
        args: Value,
    ) -> tokio::task::JoinHandle<Result<String>> {
        let registry = self.clone();
        tokio::task::spawn_blocking(move || {
            let Some(tool) = registry.get(&name) else {
                bail!("unknown tool '{name}'")
            };
            tool.execute(&args)
        })
    }

    /// One row per built-in tool; a test pins every registry name here.
    pub fn builtin_tools_info() -> Vec<BuiltinToolInfo> {
        vec![
            BuiltinToolInfo { name: "read_file", summary: "Read file contents.", approval: "auto", note: "" },
            BuiltinToolInfo { name: "write_file", summary: "Create or overwrite files.", approval: "approval", note: "" },
            BuiltinToolInfo { name: "edit_file", summary: "Exact-match search/replace edits.", approval: "approval", note: "" },
            BuiltinToolInfo { name: "find_files", summary: "Glob file search.", approval: "auto", note: "" },
            BuiltinToolInfo { name: "search_content", summary: "Regex content search.", approval: "auto", note: "" },
            BuiltinToolInfo { name: "exec", summary: "Shell commands; read-only runs free.", approval: "conditional", note: "" },
            BuiltinToolInfo { name: "spawn_subagent", summary: "Delegate to an ephemeral specialist.", approval: "auto", note: "Orchestrator only." },
            BuiltinToolInfo { name: "ask_user", summary: "Ask the user a multiple-choice question.", approval: "auto", note: "Orchestrator only." },
            BuiltinToolInfo { name: "skill", summary: "Load a learned skill.", approval: "auto", note: "" },
            BuiltinToolInfo { name: "remember", summary: "Curate long-term memory; showing it is free.", approval: "conditional", note: "" },
            BuiltinToolInfo { name: "lsp", summary: "Language-server navigation: definitions, references, symbols.", approval: "auto", note: "Requires a language server." },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("werk-tools-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn registry_for(root: &std::path::Path) -> ToolRegistry {
        let jail = Arc::new(PathJail::new(root, &[]).unwrap());
        ToolRegistry::project_tools(jail)
    }

    #[test]
    #[cfg(windows)]
    #[test]
    fn exec_returns_when_grandchild_holds_the_pipe() {
        let root = temp_dir("exec-pipe");
        let jail = Arc::new(crate::sandbox::PathJail::new(&root, &[]).unwrap());
        let tool = ExecTool { jail };
        // The parent exits at once; the grandchild inherits stdout/stderr and
        // lingers. Joining the reader threads would block until it exits.
        let command = "[System.Diagnostics.Process]::Start([System.Diagnostics.ProcessStartInfo]@{FileName='powershell';Arguments='-NoLogo -NoProfile -Command Start-Sleep 20';UseShellExecute=$false}) | Out-Null; exit 0";
        let start = std::time::Instant::now();
        let out = tool.execute(&json!({ "command": command })).unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "exec hung for {elapsed:?}: {out}"
        );
        // The grandchild still holds the directory; best-effort cleanup.
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let root = temp_dir("roundtrip");
        let reg = registry_for(&root);
        let out = reg
            .get("write_file")
            .unwrap()
            .execute(&json!({"path": "docs/a.txt", "content": "hello"}))
            .unwrap();
        assert!(out.contains("Wrote 5 bytes"));
        let out = reg.get("read_file").unwrap().execute(&json!({"path": "docs/a.txt"})).unwrap();
        assert_eq!(out, "hello");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn edit_needs_exactly_one_match() {
        let root = temp_dir("edit");
        std::fs::write(root.join("a.txt"), "one\ntwo\ntwo\n").unwrap();
        let reg = registry_for(&root);
        let edit = reg.get("edit_file").unwrap();
        assert!(edit
            .execute(&json!({"path": "a.txt", "search": "two", "replace": "2"}))
            .is_err());
        let out = edit
            .execute(&json!({"path": "a.txt", "search": "one", "replace": "1"}))
            .unwrap();
        assert!(out.contains("(+1 -1)"));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "1\ntwo\ntwo\n");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn find_skips_ignored_dirs() {
        let root = temp_dir("find");
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(root.join("node_modules/keep.log"), "").unwrap();
        std::fs::write(root.join("src_main.rs"), "").unwrap();
        let reg = registry_for(&root);
        let out = reg
            .get("find_files")
            .unwrap()
            .execute(&json!({"pattern": "**/*.rs"}))
            .unwrap();
        assert!(out.contains("src_main.rs"));
        assert!(!out.contains("keep.log"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn search_finds_regex_matches() {
        let root = temp_dir("search");
        std::fs::write(root.join("a.txt"), "first line\nneedle here\nthird\n").unwrap();
        let reg = registry_for(&root);
        let out = reg
            .get("search_content")
            .unwrap()
            .execute(&json!({"query": "needle", "pattern": "**/*.txt"}))
            .unwrap();
        assert!(out.contains("a.txt:2: needle here"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn exec_readonly_runs_free_mutating_needs_approval() {
        let root = temp_dir("exec");
        let reg = registry_for(&root);
        let exec = reg.get("exec").unwrap();
        assert!(exec.approval_key(&json!({"command": "git status"})).is_none());
        assert!(exec.approval_key(&json!({"command": "cargo test"})).is_some());
        // Read-only commands run (failure text still returns Ok).
        let out = exec.execute(&json!({"command": "echo hello"})).unwrap();
        assert!(out.contains("hello"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn repair_fixes_recoverable_shapes() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" } },
            },
            "required": ["path"]
        });
        let (out, kinds) =
            repair_tool_args(&schema, &json!({"path": "a.rs", "options": null}));
        assert!(kinds.contains(&ArgRepair::NullStripped));
        assert!(out.get("options").is_none());
        let (out, kinds) =
            repair_tool_args(&schema, &json!({"path": "a", "options": "[\"x\"]"}));
        assert!(kinds.contains(&ArgRepair::JsonStringToArray));
        assert_eq!(out["options"], json!(["x"]));
        let (out, kinds) = repair_tool_args(&schema, &json!({"path": "a", "options": "solo"}));
        assert!(kinds.contains(&ArgRepair::BareStringToArray));
        assert_eq!(out["options"], json!(["solo"]));
        let (out, kinds) = repair_tool_args(
            &schema,
            &json!({"path": "[a.rs](http://x/a.rs)"}),
        );
        assert!(kinds.contains(&ArgRepair::MdLinkUnwrapped));
        assert_eq!(out["path"], json!("a.rs"));
        let (out, kinds) = repair_tool_args(
            &schema,
            &json!({"path": "[guide](https://example.com/other)"}),
        );
        assert!(kinds.is_empty());
        assert_eq!(out["path"], json!("[guide](https://example.com/other)"));
        let (valid, kinds) = repair_tool_args(&schema, &json!({"path": "a"}));
        assert!(kinds.is_empty() && valid == json!({"path": "a"}));
    }

    #[test]
    fn inert_shell_commands_detected_narrowly() {
        for cmd in ["echo hi", "echo \"ready\"", "true", "echo a; echo b"] {
            assert!(is_inert_shell_command(cmd), "inert: {cmd}");
        }
        for cmd in ["", "cat f | grep x", "echo $HOME", "echo *.rs", "sleep 5", "git status"] {
            assert!(!is_inert_shell_command(cmd), "live: {cmd}");
        }
    }

    #[test]
    fn builtin_tools_info_covers_registry() {
        let root = temp_dir("info");
        let registry = registry_for(&root);
        let info = ToolRegistry::builtin_tools_info();
        let mut names: Vec<&str> = info.iter().map(|i| i.name).collect();
        names.sort_unstable();
        let mut dedup = names.clone();
        dedup.dedup();
        assert_eq!(names, dedup);
        for name in registry.names() {
            let rows: Vec<_> = info.iter().filter(|i| i.name == name).collect();
            assert_eq!(rows.len(), 1, "missing info row for '{name}'");
            assert!(["auto", "approval", "conditional"].contains(&rows[0].approval));
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
