//! Orchestrator loop: model ⇄ sandboxed tools, with guardrails.
//! Denials and suppressions feed back as tool results; never retry silently.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::{ChatMessage, LlmClient, StreamCollector, StreamEvent};
use crate::permissions::{ApprovalKey, Decision, PermissionEngine, Scope};
use crate::tools::ToolRegistry;

pub const DEFAULT_MAX_TURNS: usize = 40;
pub const DEFAULT_SUBAGENT_MAX_TURNS: usize = 25;

/// Shared shell guidance so workers use the right idioms.
pub fn os_shell_snippet() -> String {
    if cfg!(windows) {
        "You run on Windows: shell commands use PowerShell (Get-ChildItem, Get-Content, Select-String; statements separated by `;`) — never sh/bash (`ls -la`, `&&`, `grep`, `/dev/null`, `/` paths).".to_string()
    } else {
        "You run on a POSIX system: shell commands use sh (ls, cat, grep; `&&` or `;`) — never PowerShell.".to_string()
    }
}

// ── Subagents ─────────────────────────────────────────────────────────────

/// Same rule the orchestrator gets: shell for programs, file tools for files.
const FILE_TOOL_RULE: &str = "Use the native file tools for all file work — never use shell \
    commands to read, list, search, or edit files; `exec` is only for running programs.";

/// Subagents only see an image when the task attaches one; small models
/// otherwise confabulate visual detail from filenames and context.
const IMAGE_RULE: &str = "If your task includes an attached image, describe only what is \
    visible; never invent details — if no image is attached, say so. You cannot open image files \
    yourself.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentKind {
    Coder,
    Researcher,
}

impl SubagentKind {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_lowercase().as_str() {
            "coder" => Ok(Self::Coder),
            "researcher" => Ok(Self::Researcher),
            other => anyhow::bail!("Unknown agent type '{other}' (expected 'coder' or 'researcher')"),
        }
    }

    fn prompt(&self) -> String {
        let base: &'static str = match self {
            Self::Coder => {
                "You are a focused implementation subagent. You execute exactly one coding task inside a sandboxed project directory, then report back. \
                You cannot spawn further subagents. \
                Read relevant files before editing; make small, exact edits with edit_file; create files with write_file; \
                verify with search_content or find_files. Anything but allowlisted read-only commands needs user approval — if denied, adapt instead of retrying. \
                Do not expand the task scope. If the goal is ambiguous, make the most reasonable assumption and note it. \
                Your final message is the only thing the orchestrator sees: report what changed (files and a one-line summary each), what you verified, and anything left undone."
            }
            Self::Researcher => {
                "You are an investigation subagent. You answer exactly one question about a sandboxed project directory, then report back. \
                You cannot create or modify anything. \
                Use find_files and search_content with specific patterns; read only what is needed; verify claims by reading the actual code. \
                Your final message is the only thing the orchestrator sees: state the answer directly, with concrete file:line references as evidence, then stop. \
                When asked to verify a claim, end your report with a verdict block: `Verdict: confirmed | refuted | uncertain`, one evidence line each."
            }
        };
        format!("{base} {FILE_TOOL_RULE} {IMAGE_RULE} {}", os_shell_snippet())
    }

    fn allowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::Coder => &[
                "read_file",
                "write_file",
                "edit_file",
                "find_files",
                "search_content",
                "exec",
            ],
            Self::Researcher => &["read_file", "find_files", "search_content", "exec"],
        }
    }
}

/// True for the image extensions the chat accepts.
pub(crate) fn is_image_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp")
    )
}

/// MIME for the image extensions the chat accepts.
fn image_mime(path: &std::path::Path) -> &'static str {    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    }
}

/// The single image path a subagent goal names, if there is exactly one.
/// Small orchestrators often write "describe migu.jpg" without passing the
/// `image` argument; attaching it keeps the first spawn from being wasted.
fn image_path_in_goal(goal: &str, jail: &crate::sandbox::PathJail) -> Option<String> {
    let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
    for raw in goal.split_whitespace() {
        let token = raw
            .trim_matches(|c: char| {
                matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',')
            })
            .trim_end_matches(['.', ';', ':']);
        if !is_image_path(std::path::Path::new(token)) {
            continue;
        }
        let Ok(resolved) = jail.check_read(token) else { continue };
        if !resolved.is_file() {
            continue;
        }
        if found.iter().any(|(_, p)| *p == resolved) {
            continue;
        }
        found.push((token.to_string(), resolved));
    }
    if found.len() == 1 {
        found.pop().map(|(token, _)| token)
    } else {
        None
    }
}

/// Standard base64; keeps the harness dependency-free.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

pub struct Subagents<'a> {
    pub jail: Arc<crate::sandbox::PathJail>,
    pub max_turns: usize,
    /// None inherits the orchestrator's model.
    pub model: Option<String>,
    pub exec_enabled: bool,
    /// The worker model's own context window; None inherits the parent's.
    pub context_limit: Option<u64>,
    /// External worker provider; None uses the orchestrator's client.
    pub client: Option<&'a LlmClient>,
    /// Allowed `spawn_subagent(model=...)` overrides, validated against this.
    pub choices: Vec<SubagentChoice<'a>>,
    /// Whether the default subagent model can view images; None = unknown.
    pub vision: Option<bool>,
}

/// One named model override a subagent may request.
pub struct SubagentChoice<'a> {
    /// What the model passes (e.g. `provider:model` or `worker`).
    pub target: String,
    /// Model id to send; None means the client's default.
    pub model: Option<String>,
    /// None uses the orchestrator's client.
    pub client: Option<&'a LlmClient>,
    /// Vision capability of this choice; None = unknown (provider models).
    pub vision: Option<bool>,
    /// Context window of this choice; None inherits the subagent default.
    pub context_limit: Option<u64>,
}

impl std::fmt::Debug for SubagentChoice<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentChoice")
            .field("target", &self.target)
            .field("model", &self.model)
            .field("external_client", &self.client.is_some())
            .finish()
    }
}

/// Resolve the requested subagent model against the configured choices.
fn resolve_subagent_choice<'a>(
    choices: &'a [SubagentChoice<'a>],
    requested: Option<&str>,
) -> Result<Option<&'a SubagentChoice<'a>>> {
    let Some(name) = requested.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    match choices.iter().find(|c| c.target.eq_ignore_ascii_case(name)) {
        Some(choice) => Ok(Some(choice)),
        None if choices.is_empty() => {
            bail!("Unknown subagent model '{name}': no alternative models are configured.")
        }
        None => {
            let allowed: Vec<&str> = choices.iter().map(|c| c.target.as_str()).collect();
            bail!("Unknown subagent model '{name}'. Available: {}", allowed.join(", "))
        }
    }
}

// ── Events ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    ToolCall { call_id: String, tool: String, args: String },
    ToolResult { call_id: String, ok: bool, output: String },
    SubagentSpawned {
        call_id: String,
        kind: String,
        goal: String,
        /// Model the subagent runs on, when known (UI attribution).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    SubagentFinished { call_id: String, kind: String, summary: String },
    /// The orchestrator's task list changed; the UI renders the checkpoints.
    TodosChanged { todos: Vec<crate::todos::TodoItem> },
    /// Transcript was compacted mid-run; the UI shows how much folded away.
    Compacted { removed: usize },
    Notice { text: String },
}

// ── Approval gate ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approved {
    Denied,
    Once,
    Session,
    Project,
    Global,
}

pub struct ApprovalRequest {
    pub key: ApprovalKey,
    pub args_pretty: String,
}

/// One answer option: a short title plus an optional explanation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestionOption {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Multiple-choice question for the user (the `ask_user` tool).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestionRequest {
    pub question: String,
    pub options: Vec<QuestionOption>,
}

/// Answer to a question; `cancelled` when the run ended while parked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestionAnswer {
    pub answer: String,
    pub cancelled: bool,
}

impl QuestionAnswer {
    pub fn cancelled() -> Self {
        Self { answer: String::new(), cancelled: true }
    }
}

/// Implemented by the driver (UI event → oneshot from the answer command).
pub trait ApprovalGate: Send + Sync {
    fn decide(&self, req: ApprovalRequest) -> Pin<Box<dyn Future<Output = Approved> + Send>>;
    /// Parks until the user answers; default cancels (headless gates).
    fn ask_question(
        &self,
        _req: QuestionRequest,
    ) -> Pin<Box<dyn Future<Output = QuestionAnswer> + Send>> {
        Box::pin(async { QuestionAnswer::cancelled() })
    }
    /// Save hook after a persistable grant lands; no-op by default.
    fn grants_changed(&self, _grants: &[crate::permissions::Grant]) {}
}

// ── Verification nudges ───────────────────────────────────────────────────

/// Strictness: Off silences the detector, Normal fires on unchecked turns,
/// Thorough also fires on turns that read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum VerifyMode {
    Off,
    #[default]
    Normal,
    Thorough,
}

pub const MAX_VERIFY_NUDGES: usize = 3;
pub const MAX_VERIFY_NUDGES_THOROUGH: usize = 6;

/// Nudge cap per mode; 0 disables the detector entirely.
pub fn max_verify_nudges(mode: VerifyMode) -> usize {
    match mode {
        VerifyMode::Off => 0,
        VerifyMode::Normal => MAX_VERIFY_NUDGES,
        VerifyMode::Thorough => MAX_VERIFY_NUDGES_THOROUGH,
    }
}

/// Calls counting as "already checking" (direct reads or delegation).
fn turn_verified(tool_names: &[&str]) -> bool {
    tool_names.iter().any(|n| {
        matches!(*n, "read_file" | "find_files" | "search_content" | "spawn_subagent")
    })
}

/// Pure nudge gate: below the mode cap, never after a researcher spawn,
/// and — unless Thorough — never on turns that already read.
pub fn should_nudge(mode: VerifyMode, turn_tools: &[String], used: usize) -> bool {
    if used >= max_verify_nudges(mode) {
        return false;
    }
    if turn_tools.iter().any(|t| t == "spawn_subagent") {
        return false;
    }
    if mode == VerifyMode::Thorough {
        return true;
    }
    if mode == VerifyMode::Off {
        return false;
    }
    let refs: Vec<&str> = turn_tools.iter().map(|s| s.as_str()).collect();
    !turn_verified(&refs)
}

fn shape_excerpt(m: &str) -> String {
    let t = m.trim();
    let mut excerpt: String = t.chars().take(120).collect();
    if excerpt.len() < t.len() {
        excerpt.push('…');
    }
    excerpt
}

/// Deterministic trust-shape scan: mechanical trigger for claims worth
/// verifying, never introspective confidence (small models can't calibrate).
fn detect_trust_shape(text: &str) -> Option<(String, String)> {
    use std::sync::OnceLock;
    static SHAPES: OnceLock<Vec<(String, regex::Regex)>> = OnceLock::new();
    let shapes = SHAPES.get_or_init(|| {
        [
            ("numeric default", r"\b[a-zA-Z_]\w*\s*[:=]\s*\d+(\.\d+)?"),
            ("flag", r"\B--[a-zA-Z][\w-]*"),
            ("version", r"\bv?\d+\.\d+\.\d+\b"),
            (
                "runtime version",
                r"(?i)\b(node|python|rust|go|java|typescript)\s+\d+(\.\d+)*\b",
            ),
            (
                "universal negative",
                r"(?i)\b(has no|have no|does not (support|exist|have|work|apply)|not supported|no such|is not available)\b",
            ),
        ]
        .into_iter()
        .map(|(label, pat)| (label.to_string(), regex::Regex::new(pat).expect("trust shape regex")))
        .collect()
    });
    for (label, re) in shapes {
        if let Some(m) = re.find(text) {
            return Some((label.clone(), shape_excerpt(m.as_str())));
        }
    }
    // owner/repo without dots or slashes around it (paths are not libraries).
    // The regex crate has no look-around, so the boundary is checked by hand.
    static ENTITY: OnceLock<regex::Regex> = OnceLock::new();
    let entity = ENTITY.get_or_init(|| {
        regex::Regex::new(r"(?:^|[^.\w/])([A-Za-z0-9_-]{2,})/([A-Za-z0-9_-]{2,})")
            .expect("entity regex")
    });
    for m in entity.find_iter(text) {
        let after = text[m.end()..].chars().next();
        if matches!(after, Some(c) if c.is_alphanumeric() || c == '_' || c == '.' || c == '/') {
            continue;
        }
        return Some(("library reference".to_string(), shape_excerpt(m.as_str())));
    }
    None
}

// ── Loop guard ────────────────────────────────────────────────────────────

const REPEAT_WINDOW: usize = 8;
const MAX_SUPPRESSED_STREAK: usize = 2;

/// Guards against non-progressing loops: the 3rd identical call is
/// suppressed with a reflect-then-pivot prompt. Varied no-ops share one
/// `exec:inert` key so empty spins trip it too.
#[derive(Debug, Default)]
pub struct RepeatTracker {
    window: std::collections::VecDeque<(String, String)>,
    suppressed_streak: usize,
}

impl RepeatTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe a call key; true means suppress this run of it.
    pub fn suppress(&mut self, key: &(String, String)) -> bool {
        let seen = self.window.iter().filter(|k| *k == key).count();
        self.window.push_back(key.clone());
        while self.window.len() > REPEAT_WINDOW {
            self.window.pop_front();
        }
        seen >= 2
    }

    /// End of turn; true means exit the run (only suppressed calls, twice).
    pub fn turn_end(&mut self, all_suppressed: bool) -> bool {
        if all_suppressed {
            self.suppressed_streak += 1;
        } else {
            self.suppressed_streak = 0;
        }
        self.suppressed_streak >= MAX_SUPPRESSED_STREAK
    }
}

// ── Runner ────────────────────────────────────────────────────────────────

pub struct AgentRun<'a> {
    pub client: &'a LlmClient,
    pub registry: Arc<ToolRegistry>,
    pub engine: Arc<Mutex<PermissionEngine>>,
    pub model: Option<String>,
    pub project: Option<String>,
    pub reasoning_effort: Option<String>,
    /// Per-request sampling overrides (roleplay); None = server defaults.
    pub sampling: Option<serde_json::Value>,
    /// Whether the run's model can see images; tool results attach them only
    /// then (local models with an mmproj).
    pub vision: bool,
    pub max_turns: usize,
    pub subagents: Option<Subagents<'a>>,
    pub verify_mode: VerifyMode,
    /// Effective context size for compaction; None disables it.
    pub context_limit: Option<u64>,
    /// Cuts recorded as they happen - read after ANY outcome.
    pub compactions_log: Arc<Mutex<Vec<usize>>>,
    /// Housekeeping model for compaction: (client, model). None = the run's.
    pub utility: Option<(&'a LlmClient, Option<String>)>,
    /// The orchestrator's task list, re-injected into every request. None for
    /// subagents, which never see or edit it.
    pub todos: Option<crate::todos::TodoList>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentOutcome {
    pub text: String,
    pub gen_tokens: usize,
    pub prompt_tokens: Option<u64>,
    pub tokens_per_sec: Option<f64>,
    pub elapsed_ms: u64,
    /// Accumulated reasoning; never sent back to the model.
    pub reasoning: String,
    pub compactions: Vec<usize>,
}

fn short_args(pretty: &str) -> String {
    let one_line = pretty.lines().collect::<Vec<_>>().join(" ");
    let mut s = one_line.chars().take(300).collect::<String>();
    if s.len() < one_line.len() {
        s.push('…');
    }
    s
}

/// Tool result content: text, or text plus image parts when the model sees.
/// llama.cpp accepts image_url parts in any role (they become media markers).
fn tool_result_content(text: String, images: Vec<(String, Vec<u8>)>, vision: bool) -> Value {
    if !vision || images.is_empty() {
        return Value::String(text);
    }
    let mut parts: Vec<Value> = Vec::with_capacity(images.len() + 1);
    parts.push(json!({"type": "text", "text": text}));
    for (mime, bytes) in images {
        parts.push(json!({
            "type": "image_url",
            "image_url": {"url": format!("data:{mime};base64,{}", base64_encode(&bytes))}
        }));
    }
    Value::Array(parts)
}

/// llama-server's context-overflow rejection, matched loosely.
fn is_context_overflow(err: &str) -> bool {
    let l = err.to_lowercase();
    (l.contains("exceed") && (l.contains("context") || l.contains("token")))
        || (l.contains("context") && l.contains("overflow"))
        || l.contains("too many tokens")
        || l.contains("maximum context length")
}

impl AgentRun<'_> {
    pub async fn run(
        &self,
        history: &mut Vec<ChatMessage>,
        should_stop: Arc<dyn Fn() -> bool + Send + Sync>,
        gate: Arc<dyn ApprovalGate>,
        steer: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        mut on_stream: impl FnMut(StreamEvent) + Send,
        mut on_event: impl FnMut(AgentEvent) + Send,
    ) -> Result<AgentOutcome> {
        let mut turns_used = 0usize;
        let mut sub_seq = 0usize;
        let mut verify_nudges = 0usize;
        let mut repeats = RepeatTracker::new();
        let mut reasoning_acc = String::new();
        let mut compactions: Vec<usize> = Vec::new();
        let schema_overhead = crate::compact::schema_tokens(&self.registry.tool_schemas());
        // Compaction budget: the raw window minus what the request carries
        // outside `history`, with slack because code/JSON tokenizes denser
        // than the chars/4 estimate.
        let history_budget = |limit: u64| {
            let usable = limit.saturating_sub(schema_overhead);
            ((usable as f64 * 0.9) as u64).max(2_048.min(limit))
        };
        // Housekeeping runs (compaction) go to the utility model when set.
        let (house_client, house_model) = match &self.utility {
            Some((client, model)) => (*client, model.as_deref()),
            None => (self.client, self.model.as_deref()),
        };
        loop {
            if should_stop() {
                bail!("aborted");
            }
            // Mid-run steering joins as normal user turns (orchestrator only).
            for msg in steer() {
                history.push(ChatMessage::user(msg));
            }
            // Fold oldest turns before they overflow; free, checked each turn.
            if let Some(limit) = self.context_limit {
                if let Some(info) = crate::compact::compact_history(
                    house_client,
                    house_model,
                    history,
                    history_budget(limit),
                    false,
                    &*should_stop,
                    &mut on_event,
                )
                .await?
                {
                    if let Some(cut) = info.cut {
                        compactions.push(cut);
                        self.compactions_log.lock().unwrap().push(cut);
                    }
                }
            }
            turns_used += 1;
            if turns_used > self.max_turns {
                bail!("Turn budget exhausted ({} turns)", self.max_turns);
            }

            let mut collector = StreamCollector::default();
            let mut text_acc = String::new();
            let mut deltas = 0usize;
            let mut first_delta: Option<std::time::Instant> = None;
            let mut usage_tokens: Option<u64> = None;
            let mut usage_prompt: Option<u64> = None;
            let mut on_delta = |ev: StreamEvent| {
                match &ev {
                    StreamEvent::Content { text } => {
                        text_acc.push_str(text);
                        deltas += 1;
                    }
                    StreamEvent::ReasoningDelta { text } => {
                        reasoning_acc.push_str(text);
                        deltas += 1;
                    }
                    StreamEvent::ToolCallDelta { .. } => deltas += 1,
                    StreamEvent::Usage { prompt_tokens, completion_tokens } => {
                        usage_prompt = Some(*prompt_tokens);
                        usage_tokens = Some(*completion_tokens);
                    }
                    _ => {}
                }
                if first_delta.is_none() {
                    first_delta = Some(std::time::Instant::now());
                }
                collector.push(&ev);
                on_stream(ev);
            };
            let turn_started = std::time::Instant::now();
            // Re-inject the task list each turn (prompt state, not history) so
            // the model re-reads it instead of trusting earlier turns.
            let request: Option<Vec<ChatMessage>> = self.todo_block().map(|block| {
                let mut req = history.clone();
                let at =
                    usize::from(req.first().map(|m| m.role == "system").unwrap_or(false));
                req.insert(at, ChatMessage::system(block));
                req
            });
            // Overflow recovery: condense and retry ONCE, then surface.
            let mut finish = self
                .client
                .chat_stream(
                    self.model.as_deref(),
                    request.as_deref().unwrap_or(history),
                    Some(&self.registry.tool_schemas()),
                    self.reasoning_effort.as_deref(),
                    self.sampling.as_ref(),
                    &*should_stop,
                    &mut on_delta,
                )
                .await;
            if let Err(e) = &finish {
                if is_context_overflow(&e.to_string()) && self.context_limit.is_some() {
                    on_event(AgentEvent::Notice {
                        text: "Context overflow — compacting and retrying…".into(),
                    });
                    match crate::compact::compact_history(
                        house_client,
                        house_model,
                        history,
                        history_budget(self.context_limit.unwrap_or(u64::MAX)),
                        true,
                        &*should_stop,
                        &mut on_event,
                    )
                    .await
                    {
                        Ok(Some(info)) => {
                            if let Some(cut) = info.cut {
                                compactions.push(cut);
                                self.compactions_log.lock().unwrap().push(cut);
                            }
                            finish = self
                                .client
                                .chat_stream(
                                    self.model.as_deref(),
                                    request.as_deref().unwrap_or(history),
                                    Some(&self.registry.tool_schemas()),
                                    self.reasoning_effort.as_deref(),
                                    self.sampling.as_ref(),
                                    &*should_stop,
                                    &mut on_delta,
                                )
                                .await;
                        }
                        // Nothing to fold and the window is still too small.
                        Ok(None) => bail!(
                            "Context overflowed and there is nothing left to compact \
                             (window too small for the system prompt, tool schemas, and reply) \
                             — raise Ctx or start a new chat (/new)"
                        ),
                        Err(e) => return Err(e),
                    }
                }
            }
            let finish = finish?;
            if should_stop() {
                bail!("aborted");
            }

            let calls = collector.finish()?;
            // Reasoning that led to tool calls belongs to that turn, not the
            // final answer — reset so the footer shows only the last turn.
            if !calls.is_empty() {
                reasoning_acc.clear();
            }
            // Scrub leaked control strings from assistant text only.
            text_acc = crate::client::strip_special_tokens(&text_acc);
            reasoning_acc = crate::client::strip_special_tokens(&reasoning_acc);
            // Scan now: `text_acc` moves into the history push below.
            let trust_shape = detect_trust_shape(&text_acc);
            if calls.is_empty() {
                history.push(ChatMessage {
                    role: "assistant".into(),
                    content: Some(Value::String(text_acc.clone())),
                    tool_calls: None,
                    tool_call_id: None,
                });
                let elapsed = first_delta
                    .map(|t| t.elapsed())
                    .unwrap_or_else(|| turn_started.elapsed());
                let tokens = usage_tokens.unwrap_or(deltas as u64) as usize;
                let tokens_per_sec = if deltas > 0 {
                    Some(tokens as f64 / elapsed.as_secs_f64().max(0.001))
                } else {
                    None
                };
                let _ = finish;
                return Ok(AgentOutcome {
                    text: text_acc,
                    gen_tokens: tokens,
                    prompt_tokens: usage_prompt,
                    tokens_per_sec: tokens_per_sec.filter(|v| *v > 0.0 && v.is_finite()),
                    elapsed_ms: elapsed.as_millis() as u64,
                    reasoning: std::mem::take(&mut reasoning_acc),
                    compactions,
                });
            }

            // Assistant message with tool calls precedes tool results.
            history.push(ChatMessage {
                role: "assistant".into(),
                content: if text_acc.is_empty() { None } else { Some(Value::String(text_acc.clone())) },
                tool_calls: Some(calls.clone()),
                tool_call_id: None,
            });

            // Names for the checks below (`calls` moves into the loop).
            let turn_tools: Vec<String> =
                calls.iter().map(|c| c.function.name.clone()).collect();
            let n_calls = calls.len();
            let mut suppressed_this_turn = 0usize;
            for call in calls {
                if should_stop() {
                    bail!("aborted");
                }
                let call_id = call.id.clone();
                let tool_name = call.function.name.clone();
                let args_value: serde_json::Value =
                    serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
                let args_pretty =
                    serde_json::to_string_pretty(&args_value).unwrap_or_else(|_| call.function.arguments.clone());
                on_event(AgentEvent::ToolCall {
                    call_id: call_id.clone(),
                    tool: tool_name.clone(),
                    args: short_args(&args_pretty),
                });

                // Loop guard: the 3rd identical call is skipped with a
                // reflect-then-pivot prompt instead of executing.
                let storm_key = if tool_name == "exec"
                    && args_value
                        .get("command")
                        .and_then(|c| c.as_str())
                        .map(crate::tools::is_inert_shell_command)
                        .unwrap_or(false)
                {
                    ("exec".to_string(), "inert".to_string())
                } else {
                    (tool_name.clone(), call.function.arguments.clone())
                };
                if repeats.suppress(&storm_key) {
                    suppressed_this_turn += 1;
                    let text: &str = if storm_key.1 == "inert" {
                        "[loop guard] That command changes nothing and returns no new information, so it was skipped. Use a tool that can actually move the task forward."
                    } else {
                        "[loop guard] This exact call already ran twice without moving the task forward, so it was skipped. State what you are trying to achieve and why it isn't working, name the assumption that might be wrong, then try 2-3 fundamentally different approaches (a different tool, entry point, or interpretation) and proceed with one. If nothing can work with the available tools, say so plainly instead of retrying."
                    };
                    on_event(AgentEvent::ToolResult {
                        call_id: call_id.clone(),
                        ok: false,
                        output: short_args(text),
                    });
                    history.push(ChatMessage {
                        role: "tool".into(),
                        content: Some(Value::String(text.to_string())),
                        tool_calls: None,
                        tool_call_id: Some(call_id),
                    });
                    continue;
                }

                // `spawn_subagent` runs inline; only its report enters history.
                let output = if tool_name == "spawn_subagent" {
                    match &self.subagents {
                        None => "error: subagents are not available".to_string(),
                        Some(sub) => {
                            let seq = sub_seq;
                            sub_seq += 1;
                            match Box::pin(self
                                .run_subagent(sub, seq, &args_value, gate.clone(), should_stop.clone(), &mut on_event))
                                .await
                            {
                                Ok(report) => report,
                                Err(e) => format!("error: {e:#}"),
                            }
                        }
                    }
                } else {
                    let output = self.execute_tool_call(&tool_name, &args_value, args_pretty, &call_id, &gate, &mut on_event)
                        .await;
                    if tool_name == "todo" {
                        if let Some(list) = &self.todos {
                            on_event(AgentEvent::TodosChanged {
                                todos: list.lock().unwrap().clone(),
                            });
                        }
                    }
                    let images = self
                        .registry
                        .get(&tool_name)
                        .map(|tool| tool.result_images(&args_value, &output))
                        .unwrap_or_default();
                    // Secrets never enter the model context, whatever the tool.
                    let output = crate::sensitive::redact_secrets(&output);
                    history.push(ChatMessage {
                        role: "tool".into(),
                        content: Some(tool_result_content(output, images, self.vision)),
                        tool_calls: None,
                        tool_call_id: Some(call_id),
                    });
                    continue;
                };

                history.push(ChatMessage {
                    role: "tool".into(),
                    content: Some(Value::String(crate::sensitive::redact_secrets(&output))),
                    tool_calls: None,
                    tool_call_id: Some(call_id),
                });
            }
            // Two consecutive fully-suppressed turns end the run gracefully.
            if repeats.turn_end(suppressed_this_turn == n_calls) {
                let elapsed = first_delta
                    .map(|t| t.elapsed())
                    .unwrap_or_else(|| turn_started.elapsed());
                let tokens = usage_tokens.unwrap_or(deltas as u64) as usize;
                let tokens_per_sec = if deltas > 0 {
                    Some(tokens as f64 / elapsed.as_secs_f64().max(0.001))
                } else {
                    None
                };
                let stopped = "(stopped here: the last turns only repeated calls without progress)";
                return Ok(AgentOutcome {
                    text: if text_acc.trim().is_empty() {
                        stopped.to_string()
                    } else {
                        format!("{text_acc}\n\n{stopped}")
                    },
                    gen_tokens: tokens,
                    prompt_tokens: usage_prompt,
                    tokens_per_sec: tokens_per_sec.filter(|v| *v > 0.0 && v.is_finite()),
                    elapsed_ms: elapsed.as_millis() as u64,
                    reasoning: std::mem::take(&mut reasoning_acc),
                    compactions,
                });
            }
            // Verify-before-acting nudge, gated by strictness mode.
            if should_nudge(self.verify_mode, &turn_tools, verify_nudges) {
                if let Some((shape, excerpt)) = trust_shape {
                    verify_nudges += 1;
                    let cap = max_verify_nudges(self.verify_mode);
                    history.push(ChatMessage {
                        role: "user".into(),
                        content: Some(Value::String(format!(
                            "Verify-before-acting nudge ({verify_nudges}/{cap} this run): \
                            your message states a technical claim from memory ({shape}: \"{excerpt}\"). \
                            If you have not verified it this session — one direct read or search when the \
                            answer lives in this project, otherwise a single researcher subagent — do so \
                            before acting on it. Skip this when the claim is trivial or already checked."
                        ))),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }
        }
    }

    /// Rendered task list for the per-turn prompt injection.
    fn todo_block(&self) -> Option<String> {
        let list = self.todos.as_ref()?;
        let items = list.lock().unwrap();
        crate::todos::render_block(&items)
    }

    async fn execute_tool_call(
        &self,
        tool_name: &str,
        args_value: &Value,
        args_pretty: String,
        call_id: &str,
        gate: &Arc<dyn ApprovalGate>,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> String {
        let Some(tool) = self.registry.get(tool_name) else {
            return format!("error: unknown tool '{tool_name}'");
        };
        // Recoverable arg shapes are fixed before approval and execution.
        let mut args_value = args_value.clone();
        let mut args_pretty = args_pretty;
        {
            let (repaired, kinds) =
                crate::tools::repair_tool_args(&tool.parameters(), &args_value);
            if !kinds.is_empty() {
                args_value = repaired;
                args_pretty =
                    serde_json::to_string_pretty(&args_value).unwrap_or(args_pretty);
                let kinds = kinds
                    .iter()
                    .map(|k| format!("{k:?}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                on_event(AgentEvent::Notice {
                    text: format!("Repaired {tool_name} args ({kinds})."),
                });
            }
        }
        // `ask_user` parks on the gate; registries without it stay unknown.
        if tool_name == "ask_user" {
            return self.ask_user(&args_value, call_id, gate, on_event).await;
        }
        let project = self.project.as_deref();
        let allowed = match tool.approval_key(&args_value) {
            None => true,
            Some(key) => {
                // Scoped lock: the guard must not live across awaits below.
                let decision = self.engine.lock().unwrap().check(&key, project);
                match decision {
                Decision::Allowed => true,
                Decision::NeedsApproval => match gate
                    .decide(ApprovalRequest {
                        args_pretty: args_pretty.clone(),
                        key: key.clone(),
                    })
                    .await
                {
                    Approved::Denied => false,
                    scope => {
                        let grants = {
                            let mut engine = self.engine.lock().unwrap();
                            engine.grant(
                                &key.tool,
                                key.command.as_deref(),
                                match scope {
                                    Approved::Once => Scope::Once,
                                    Approved::Session => Scope::Session,
                                    Approved::Project => Scope::Project,
                                    Approved::Global => Scope::Global,
                                    Approved::Denied => unreachable!(),
                                },
                                self.project.clone(),
                            );
                            engine.persistable()
                        };
                        // The callback may re-enter the engine (the app persists
                        // grants to disk), so the lock must be dropped first.
                        gate.grants_changed(&grants);
                        self.engine.lock().unwrap().check(&key, project) == Decision::Allowed
                    }
                }
            }
            }
        };
        if !allowed {
            on_event(AgentEvent::ToolResult {
                call_id: call_id.to_string(),
                ok: false,
                output: "denied by user".into(),
            });
            return "denied by user".to_string();
        }
        // Tool results are text-only in M1 (multimodal lands with attachments).
        match self
            .registry
            .spawn_execute(tool_name.to_string(), args_value)
            .await
        {
            Ok(Ok(out)) => {
                on_event(AgentEvent::ToolResult {
                    call_id: call_id.to_string(),
                    ok: true,
                    output: short_args(&out),
                });
                out
            }
            Ok(Err(e)) => {
                let msg = format!("error: {e:#}");
                on_event(AgentEvent::ToolResult {
                    call_id: call_id.to_string(),
                    ok: false,
                    output: short_args(&msg),
                });
                msg
            }
            Err(join) => {
                let msg = format!("error: tool task failed: {join}");
                on_event(AgentEvent::ToolResult {
                    call_id: call_id.to_string(),
                    ok: false,
                    output: short_args(&msg),
                });
                msg
            }
        }
    }

    async fn ask_user(
        &self,
        args_value: &Value,
        call_id: &str,
        gate: &Arc<dyn ApprovalGate>,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> String {
        let question = args_value
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let options: Vec<QuestionOption> = args_value
            .get("options")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| match v {
                        // Bare strings are a tolerated legacy shape.
                        Value::String(s) => {
                            let title = s.trim().to_string();
                            (!title.is_empty()).then_some(QuestionOption {
                                title,
                                description: None,
                            })
                        }
                        Value::Object(m) => {
                            let title =
                                m.get("title").and_then(|t| t.as_str())?.trim().to_string();
                            if title.is_empty() {
                                return None;
                            }
                            let description = m
                                .get("description")
                                .and_then(|d| d.as_str())
                                .map(|d| d.trim().to_string())
                                .filter(|d| !d.is_empty());
                            Some(QuestionOption { title, description })
                        }
                        _ => None,
                    })
                    .take(6)
                    .collect()
            })
            .unwrap_or_default();
        let (text, ok) = if question.is_empty() {
            ("error: ask_user needs a non-empty 'question'".to_string(), false)
        } else if options.len() < 2 {
            ("error: ask_user needs at least 2 non-empty 'options'".to_string(), false)
        } else {
            let ans = gate
                .ask_question(QuestionRequest { question, options })
                .await;
            if ans.cancelled || ans.answer.trim().is_empty() {
                ("The question was dismissed before the user answered — proceed with your best judgment.".to_string(), false)
            } else {
                (format!("User's answer: {}", ans.answer.trim()), true)
            }
        };
        on_event(AgentEvent::ToolResult {
            call_id: call_id.to_string(),
            ok,
            output: short_args(&text),
        });
        text
    }

    async fn run_subagent(
        &self,
        sub: &Subagents<'_>,
        seq: usize,
        args_value: &Value,
        gate: Arc<dyn ApprovalGate>,
        should_stop: Arc<dyn Fn() -> bool + Send + Sync>,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<String> {
        let goal = args_value
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if goal.is_empty() {
            bail!("spawn_subagent needs a non-empty 'goal'");
        }
        let kind = SubagentKind::parse(
            args_value.get("agent_type").and_then(|v| v.as_str()).unwrap_or(""),
        )?;
        let choice = resolve_subagent_choice(
            &sub.choices,
            args_value.get("model").and_then(|v| v.as_str()),
        )?;
        let vision = choice.and_then(|c| c.vision).or(sub.vision);
        let explicit_image = args_value
            .get("image")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string);
        // Small orchestrators forget the argument; when the goal names exactly
        // one existing image and the model can see, attach it anyway.
        let mut auto_image = false;
        let image_arg = match explicit_image {
            Some(path) => Some(path),
            None if vision == Some(true) => match image_path_in_goal(&goal, &sub.jail) {
                Some(path) => {
                    auto_image = true;
                    Some(path)
                }
                None => None,
            },
            None => None,
        };
        let image_url = match &image_arg {
            None => None,
            Some(path) => {
                if vision == Some(false) {
                    bail!(
                        "The subagent's model has no vision — describe the image in the main \
                        chat, or pick a vision-capable subagent model."
                    );
                }
                let resolved = sub.jail.check_read(path)?;
                let bytes = std::fs::read(&resolved)
                    .with_context(|| format!("Cannot read image {}", resolved.display()))?;
                const IMAGE_CAP: usize = 8 * 1024 * 1024;
                if bytes.len() > IMAGE_CAP {
                    bail!("Image too large ({} bytes; 8 MiB max)", bytes.len());
                }
                Some(format!(
                    "data:{};base64,{}",
                    image_mime(&resolved),
                    base64_encode(&bytes)
                ))
            }
        };
        // Name the file in the task text: the model sees the pixels, but a
        // path keeps the task unambiguous when the goal says "the image".
        let task_text = match &image_arg {
            Some(path) if auto_image => {
                format!("{goal}\n\n[Attached image: {path} (auto-attached from the goal)]")
            }
            Some(path) => format!("{goal}\n\n[Attached image: {path}]"),
            None => goal.clone(),
        };
        let (run_client, run_model) = match choice {
            Some(c) => (
                c.client.unwrap_or(self.client),
                c.model.clone().or_else(|| self.model.clone()),
            ),
            None => (
                sub.client.unwrap_or(self.client),
                sub.model.clone().or_else(|| self.model.clone()),
            ),
        };
        on_event(AgentEvent::SubagentSpawned {
            call_id: format!("sub{seq}"),
            kind: format!("{kind:?}").to_lowercase(),
            goal: short_args(&goal),
            model: run_model.clone(),
        });
        let mut registry =
            ToolRegistry::project_tools(sub.jail.clone()).without(&["spawn_subagent"]);
        if !sub.exec_enabled {
            registry = registry.without(&["exec"]);
        }
        let refs: Vec<&str> = kind.allowed_tools().to_vec();
        let registry = Arc::new(registry.only(&refs));
        let mut history = vec![ChatMessage::system(kind.prompt())];
        history.push(match image_url {
            Some(url) => ChatMessage {
                role: "user".into(),
                content: Some(json!([
                    {"type": "text", "text": task_text},
                    {"type": "image_url", "image_url": {"url": url}},
                ])),
                tool_calls: None,
                tool_call_id: None,
            },
            None => ChatMessage::user(task_text),
        });
        let run = AgentRun {
            client: run_client,
            registry,
            engine: self.engine.clone(),
            model: run_model,
            project: self.project.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            sampling: None,
            vision: vision.unwrap_or(false),
            max_turns: sub.max_turns,
            subagents: None,
            verify_mode: self.verify_mode,
            context_limit: choice
                .and_then(|c| c.context_limit)
                .or(sub.context_limit)
                .or(self.context_limit),
            compactions_log: Arc::new(Mutex::new(Vec::new())),
            utility: self.utility.clone(),
            // Subagents never see or edit the orchestrator's task list.
            todos: None,
        };
        let no_steer: Arc<dyn Fn() -> Vec<String> + Send + Sync> =
            Arc::new(Vec::new);
        let mut noop = |_ev: StreamEvent| {};
        let mut nested = |ev: AgentEvent| {
            // Subagent tool traffic is namespaced by the spawn it belongs to,
            // so the parent transcript can attribute each card to its model.
            let ev = match ev {
                AgentEvent::ToolCall { call_id, tool, args } => AgentEvent::ToolCall {
                    call_id: format!("sub{seq}:{call_id}"),
                    tool,
                    args,
                },
                AgentEvent::ToolResult { call_id, ok, output } => AgentEvent::ToolResult {
                    call_id: format!("sub{seq}:{call_id}"),
                    ok,
                    output,
                },
                other => other,
            };
            on_event(ev);
        };
        let outcome = run
            .run(&mut history, should_stop, gate, no_steer, &mut noop, &mut nested)
            .await?;
        let report = outcome.text;
        const REPORT_CAP: usize = 16_000;
        let mut report = if report.chars().count() > REPORT_CAP {
            let n = report.chars().count();
            let mut short: String = report.chars().take(REPORT_CAP).collect();
            short.push_str(&format!("\n[…truncated — {} more characters]", n - REPORT_CAP));
            short
        } else {
            report
        };
        report = format!("Subagent ({}) report:\n{}", format!("{kind:?}").to_lowercase(), report);
        on_event(AgentEvent::SubagentFinished {
            call_id: format!("sub{seq}"),
            kind: format!("{kind:?}").to_lowercase(),
            summary: short_args(&report),
        });
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DenyGate;
    impl ApprovalGate for DenyGate {
        fn decide(&self, _req: ApprovalRequest) -> Pin<Box<dyn Future<Output = Approved> + Send>> {
            Box::pin(async { Approved::Denied })
        }
    }

    struct AnswerGate {
        answer: String,
        seen: std::sync::Mutex<Option<QuestionRequest>>,
    }
    impl ApprovalGate for AnswerGate {
        fn decide(&self, _req: ApprovalRequest) -> Pin<Box<dyn Future<Output = Approved> + Send>> {
            Box::pin(async { Approved::Denied })
        }
        fn ask_question(
            &self,
            req: QuestionRequest,
        ) -> Pin<Box<dyn Future<Output = QuestionAnswer> + Send>> {
            let answer = self.answer.clone();
            *self.seen.lock().unwrap() = Some(req);
            Box::pin(async { QuestionAnswer { answer, cancelled: false } })
        }
    }

    #[test]
    fn tool_results_attach_images_only_with_vision() {
        let images = vec![("image/png".to_string(), vec![1u8, 2, 3])];
        assert!(tool_result_content("hi".into(), Vec::new(), true).is_string());
        assert!(tool_result_content("hi".into(), images.clone(), false).is_string());
        let on = tool_result_content("hi".into(), images, true);
        let parts = on.as_array().expect("content parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        let url = parts[1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
    }

    fn test_run(jail: Arc<crate::sandbox::PathJail>) -> AgentRun<'static> {
        // Leaked client never touches the network (read_file is local).
        let client: &'static LlmClient = Box::leak(Box::new(LlmClient::new("http://127.0.0.1:9")));
        AgentRun {
            client,
            registry: Arc::new(ToolRegistry::project_tools(jail)),
            engine: Arc::new(Mutex::new(PermissionEngine::new())),
            model: None,
            project: Some("p".to_string()),
            reasoning_effort: None,
            sampling: None,
            vision: false,
            max_turns: 5,
            subagents: None,
            verify_mode: VerifyMode::Normal,
            context_limit: None,
            compactions_log: Arc::new(Mutex::new(Vec::new())),
            utility: None,
            todos: None,
        }
    }

    struct ProbeGate {
        engine: Arc<Mutex<PermissionEngine>>,
        saw_unlocked: Arc<std::sync::atomic::AtomicBool>,
    }

    impl ApprovalGate for ProbeGate {
        fn decide(&self, _req: ApprovalRequest) -> Pin<Box<dyn Future<Output = Approved> + Send>> {
            Box::pin(async { Approved::Project })
        }
        fn grants_changed(&self, grants: &[crate::permissions::Grant]) {
            if !grants.is_empty() {
                self.saw_unlocked.store(
                    self.engine.try_lock().is_ok(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }
        }
    }

    #[tokio::test]
    async fn grants_callback_runs_without_the_engine_lock() {
        let dir = std::env::temp_dir().join(format!("werk-grant-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let jail = Arc::new(crate::sandbox::PathJail::new(&dir, &[]).unwrap());
        let engine = Arc::new(Mutex::new(PermissionEngine::new()));
        let saw = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate: Arc<dyn ApprovalGate> = Arc::new(ProbeGate {
            engine: engine.clone(),
            saw_unlocked: saw.clone(),
        });
        let mut run = test_run(jail);
        run.engine = engine;
        let mut noop = |_ev: AgentEvent| {};
        let out = run
            .execute_tool_call(
                "write_file",
                &serde_json::json!({"path": "probe.txt", "content": "hi"}),
                "{}".to_string(),
                "c1",
                &gate,
                &mut noop,
            )
            .await;
        assert!(out.starts_with("Wrote"), "write failed: {out}");
        assert!(
            saw.load(std::sync::atomic::Ordering::SeqCst),
            "grants callback ran while the engine lock was held"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nudge_gate_follows_strictness_mode() {
        let s = |t: &str| t.to_string();
        assert!(!should_nudge(VerifyMode::Off, &[s("write_file")], 0));
        assert!(should_nudge(VerifyMode::Normal, &[s("write_file")], 0));
        assert!(should_nudge(VerifyMode::Normal, &[], 2));
        assert!(!should_nudge(VerifyMode::Normal, &[s("write_file")], 3));
        assert!(!should_nudge(VerifyMode::Normal, &[s("read_file")], 0));
        assert!(!should_nudge(VerifyMode::Normal, &[s("spawn_subagent")], 0));
        assert!(should_nudge(VerifyMode::Thorough, &[s("read_file")], 0));
        assert!(should_nudge(VerifyMode::Thorough, &[s("write_file")], 5));
        assert!(!should_nudge(VerifyMode::Thorough, &[s("write_file")], 6));
        assert!(!should_nudge(VerifyMode::Thorough, &[s("spawn_subagent")], 0));
        assert_eq!(max_verify_nudges(VerifyMode::Off), 0);
        assert_eq!(max_verify_nudges(VerifyMode::Normal), 3);
        assert_eq!(max_verify_nudges(VerifyMode::Thorough), 6);
    }

    #[test]
    fn trust_shapes_match_claims_not_prose() {
        let hit = [
            ("The default is retries=10", "numeric default"),
            ("pass --model qwen to the server", "flag"),
            ("requires v8.0.1 or later", "version"),
            ("works on Node 20 and up", "runtime version"),
            ("fetch() has no timeout option", "universal negative"),
            ("see sindresorhus/p-retry for details", "library reference"),
        ];
        for (text, shape) in hit {
            let got = detect_trust_shape(text);
            assert_eq!(got.map(|(s, _)| s), Some(shape.to_string()), "{text}");
        }
        for text in [
            "I'll check the config on port 8080 first",
            "status 200 means it worked",
            "I always run the tests before committing",
            "read src/main.rs for the entry point",
            "use a -- b as a separator here",
            "the 16:9 aspect looks fine",
            "hello world, doing the thing now",
        ] {
            assert_eq!(detect_trust_shape(text), None, "{text}");
        }
    }

    #[test]
    fn repeat_tracker_suppresses_third_identical_call() {
        let mut t = RepeatTracker::new();
        let key = ("read_file".to_string(), r#"{"path":"a"}"#.to_string());
        let other = ("read_file".to_string(), r#"{"path":"b"}"#.to_string());
        assert!(!t.suppress(&key));
        assert!(!t.suppress(&other));
        assert!(!t.suppress(&key));
        assert!(t.suppress(&key));
        for i in 0..8 {
            let k = ("read_file".to_string(), format!("{{\"path\":\"{i}\"}}"));
            assert!(!t.suppress(&k));
        }
        assert!(!t.suppress(&key), "evicted from window");
    }

    #[test]
    fn base64_encodes_images_and_mime_maps() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
        let p = |s: &str| image_mime(std::path::Path::new(s));
        assert_eq!(p("a.JPG"), "image/jpeg");
        assert_eq!(p("a.webp"), "image/webp");
        assert_eq!(p("a.gif"), "image/gif");
        assert_eq!(p("a.png"), "image/png");
        assert_eq!(p("noext"), "image/png");
    }

    #[test]
    fn goal_image_auto_attach_is_narrow() {
        let dir = std::env::temp_dir().join(format!("werk-autoimg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("migu.jpg"), b"x").unwrap();
        std::fs::write(dir.join("other.png"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let jail = crate::sandbox::PathJail::new(&dir, &[]).unwrap();

        let hit = image_path_in_goal("Describe the image migu.jpg, then stop.", &jail);
        assert_eq!(hit.as_deref(), Some("migu.jpg"));
        // The same file named twice (relative and absolute) is one image.
        let both = format!("look at migu.jpg and {}", dir.join("migu.jpg").display());
        assert_eq!(image_path_in_goal(&both, &jail).as_deref(), Some("migu.jpg"));
        // Two different images are ambiguous; non-images and outside paths
        // never auto-attach.
        assert_eq!(image_path_in_goal("compare migu.jpg with other.png", &jail), None);
        assert_eq!(image_path_in_goal("read notes.txt", &jail), None);
        assert_eq!(image_path_in_goal(r"describe C:\outside\x.jpg", &jail), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn subagent_model_choices_validate_and_resolve() {
        let choices = vec![
            SubagentChoice {
                target: "provider:big".into(),
                model: Some("big".into()),
                client: None,
                vision: None,
                context_limit: None,
            },
            SubagentChoice {
                target: "worker".into(),
                model: Some("w-1".into()),
                client: None,
                vision: Some(false),
                context_limit: Some(8_192),
            },
        ];
        assert!(resolve_subagent_choice(&choices, None).unwrap().is_none());
        assert!(resolve_subagent_choice(&choices, Some("  ")).unwrap().is_none());
        let hit = resolve_subagent_choice(&choices, Some("WORKER")).unwrap().unwrap();
        assert_eq!(hit.model.as_deref(), Some("w-1"));
        let err = resolve_subagent_choice(&choices, Some("ghost")).unwrap_err().to_string();
        assert!(err.contains("provider:big") && err.contains("worker"), "{err}");
        let err = resolve_subagent_choice(&[], Some("ghost")).unwrap_err().to_string();
        assert!(err.contains("no alternative models"), "{err}");
    }

    #[test]
    fn repeat_tracker_exits_after_suppressed_streak() {
        let mut t = RepeatTracker::new();
        assert!(!t.turn_end(false));
        assert!(!t.turn_end(true));
        assert!(t.turn_end(true));
        assert!(!t.turn_end(false));
        assert!(!t.turn_end(true));
    }

    #[test]
    fn ask_user_returns_gate_answer() {
        let dir = std::env::temp_dir().join(format!("werk-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let jail = Arc::new(crate::sandbox::PathJail::new(&dir, &[]).unwrap());
        let gate = Arc::new(AnswerGate {
            answer: "the second option".to_string(),
            seen: std::sync::Mutex::new(None),
        });
        let dyn_gate: Arc<dyn ApprovalGate> = gate.clone();
        let args = serde_json::json!({"question": "Which way?", "options": [
            {"title": "first", "description": "the short road"},
            {"title": "second"},
            "bare legacy option",
        ]});
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let run = test_run(jail);
        let mut events = Vec::new();
        let text = rt.block_on(async {
            let mut push = |ev: AgentEvent| events.push(ev);
            run.ask_user(&args, "q1", &dyn_gate, &mut push).await
        });
        assert!(text.contains("the second option"), "{text}");
        assert!(
            events.iter().any(|e| matches!(e, AgentEvent::ToolResult { call_id, ok: true, .. } if call_id == "q1")),
            "{events:?}"
        );
        let seen = gate.seen.lock().unwrap().clone().unwrap();
        assert_eq!(seen.options.len(), 3);
        assert_eq!(seen.options[0].title, "first");
        assert_eq!(seen.options[0].description.as_deref(), Some("the short road"));
        assert_eq!(seen.options[1].title, "second");
        assert_eq!(seen.options[1].description, None);
        assert_eq!(seen.options[2].title, "bare legacy option");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ask_user_validates_question_and_options() {
        let dir = std::env::temp_dir().join(format!("werk-ask-v-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let jail = Arc::new(crate::sandbox::PathJail::new(&dir, &[]).unwrap());
        let gate: Arc<dyn ApprovalGate> = Arc::new(DenyGate);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let run = test_run(jail);
        for args in [
            serde_json::json!({"question": "", "options": [{"title": "a"}, {"title": "b"}]}),
            serde_json::json!({"question": "Which?", "options": [{"title": "only"}]}),
            serde_json::json!({"question": "Which?"}),
            serde_json::json!({"question": "Which?", "options": [{"description": "no title"}, {"title": "b"}]}),
        ] {
            let mut events = Vec::new();
            let text = rt.block_on(async {
                let mut push = |ev: AgentEvent| events.push(ev);
                run.ask_user(&args, "q9", &gate, &mut push).await
            });
            assert!(text.starts_with("error: ask_user"), "{text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
