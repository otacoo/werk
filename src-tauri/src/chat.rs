//! Harness driver: Tauri commands around the agent loop.
//! Events go over the global bus (`harness_event`, `harness_approval`,
//! `harness_question`); runs are exclusive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::config::HarnessProject;
use crate::AppState;
use harness::agent::{
    AgentEvent, AgentRun, ApprovalGate, ApprovalRequest, Approved, QuestionAnswer,
    QuestionRequest,
};
use harness::client::{ChatMessage, LlmClient, StreamEvent};
use harness::permissions::PermissionEngine;
use harness::sandbox::PathJail;
use harness::tools::ToolRegistry;

// ── Runtime state ─────────────────────────────────────────────────────────

pub struct HarnessRuntime {
    /// Live conversation (system prompt included once).
    pub history: Mutex<Vec<ChatMessage>>,
    pub engine: Arc<Mutex<PermissionEngine>>,
    /// Set while the loop is in flight; blocks concurrent sends.
    pub running: std::sync::atomic::AtomicBool,
    /// Parked approval/question the loop waits on (oneshot per request).
    pub pending: Mutex<Option<tokio::sync::oneshot::Sender<Approved>>>,
    pub pending_question: Mutex<Option<tokio::sync::oneshot::Sender<QuestionAnswer>>>,
    /// Mid-run steering, drained at the next turn boundary.
    pub steering: Mutex<Vec<String>>,
    /// Open session id (None = new on next send).
    pub session_id: Mutex<Option<String>>,
    /// Display metadata by message index (footer stats).
    pub meta: Mutex<HashMap<usize, MessageMeta>>,
}

impl HarnessRuntime {
    pub fn new() -> Self {
        Self {
            history: Mutex::new(Vec::new()),
            engine: Arc::new(Mutex::new(PermissionEngine::new())),
            running: std::sync::atomic::AtomicBool::new(false),
            pending: Mutex::new(None),
            pending_question: Mutex::new(None),
            steering: Mutex::new(Vec::new()),
            session_id: Mutex::new(None),
            meta: Mutex::new(HashMap::new()),
        }
    }
}

/// Footer stats for one assistant message. Integers are 32-bit: exact in
/// JSON and far beyond physical counts (u32 tokens, unix seconds to 2106).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct MessageMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_per_sec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gen_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u32>,
    /// Accumulated reasoning; shown in a collapsible block, never resent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Files the agent changed during the run that produced this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<RunChanges>,
}

/// One file the agent changed in a run, with git line counts.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RunChange {
    pub path: String,
    pub status: String,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RunChanges {
    pub files: Vec<RunChange>,
    pub added: u32,
    pub removed: u32,
}

/// Agent run result: text plus the metadata shown under each response.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct RunResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_per_sec: Option<f64>,
    pub gen_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    pub elapsed_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<RunChanges>,
}

// ── System prompt ─────────────────────────────────────────────────────────

/// Router mode has a worker model to delegate to; single and external modes
/// don't, so their prompt never suggests subagents.
fn verify_protocol(delegate: bool) -> String {
    let fallback = if delegate {
        "otherwise a single researcher subagent"
    } else {
        "otherwise one direct read or search"
    };
    format!(
        "Verify load-bearing claims from memory before acting on them: when you are about to use an API \
        default, flag, or version behavior you recall from training (not something you read this session), \
        check it first — one direct read or search when the answer lives in this project, {fallback}. \
        Never verify trivia, never verify the same fact twice (save verified facts with the remember tool), \
        and never let verification stall the task: one check, then proceed. "
    )
}

fn system_prompt_for(
    mode: harness::agent::VerifyMode,
    server_mode: crate::config::ServerMode,
) -> String {
    let verify = if mode == harness::agent::VerifyMode::Off {
        String::new()
    } else {
        verify_protocol(server_mode == crate::config::ServerMode::Router)
    };
    format!(
        "You are Werk's agent, working inside a sandboxed project directory. \
        File tools are rooted at that directory; relative paths resolve there. \
        {} \
        Prefer the native file tools (read_file, find_files, search_content) over shell listing/searching. \
        Read-only operations run automatically; writes and shell commands may require user approval — \
        if denied, adapt instead of retrying the same call. \
        Learn across sessions: when the user states a durable preference or corrects you, save it with the \
        remember tool (project scope unless it is about the user themselves). Keep memories short. \
        When you genuinely need the user's input to proceed — an ambiguous requirement, a fork in the plan — \
        ask one focused multiple-choice question with the ask_user tool (2-4 options); never ask about \
        facts you can look up yourself. \
        {verify}\
        Work step by step: read before editing, make small exact edits, verify results, \
        and give a concise summary when done.",
        harness::agent::os_shell_snippet()
    )
}

// ── Approval gate ─────────────────────────────────────────────────────────

struct UiGate {
    app: AppHandle,
    runtime: Arc<HarnessRuntime>,
}

impl ApprovalGate for UiGate {
    fn decide(
        &self,
        req: ApprovalRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Approved> + Send>> {
        let app = self.app.clone();
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            {
                let mut pending = runtime.pending.lock().unwrap();
                if let Some(old) = pending.take() {
                    let _ = old.send(Approved::Denied);
                }
                *pending = Some(tx);
            }
            let _ = app.emit(
                "harness_approval",
                serde_json::json!({
                    "type": "approval_required",
                    "tool": req.key.tool,
                    "command": req.key.command,
                    "args": req.args_pretty,
                }),
            );
            rx.await.unwrap_or(Approved::Denied)
        })
    }

    fn ask_question(
        &self,
        req: QuestionRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = QuestionAnswer> + Send>> {
        let app = self.app.clone();
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            {
                let mut pending = runtime.pending_question.lock().unwrap();
                if let Some(old) = pending.take() {
                    let _ = old.send(QuestionAnswer::cancelled());
                }
                *pending = Some(tx);
            }
            let _ = app.emit(
                "harness_question",
                serde_json::json!({
                    "type": "question_asked",
                    "question": req.question,
                    "options": req.options,
                }),
            );
            rx.await.unwrap_or_else(|_| QuestionAnswer::cancelled())
        })
    }

    fn grants_changed(&self, grants: &[harness::permissions::Grant]) {
        if grants.iter().any(|g| g.scope.persistable()) {
            let state: tauri::State<'_, AppState> = self.app.state();
            save_permission_grants(&state);
        }
    }
}

fn grants_path() -> Option<PathBuf> {
    crate::config::data_dir().map(|d| d.join("werk").join("grants.json"))
}

fn save_permission_grants(state: &AppState) {
    let Some(path) = grants_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // try_lock: a contended lock must defer the write, never freeze the app.
    if let Ok(grants) = state.harness.engine.try_lock().map(|e| e.persistable()) {
        if let Ok(json) = serde_json::to_string_pretty(&grants) {
            let _ = std::fs::write(&path, json);
        }
    }
}

fn load_permission_grants(state: &AppState) {
    let Some(path) = grants_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    if let Ok(grants) = serde_json::from_str(&text) {
        if let Ok(mut engine) = state.harness.engine.lock() {
            engine.load_persisted(grants);
        }
    }
}

// ── Registry + roles ──────────────────────────────────────────────────────

/// Native tools + memory curator + skills + file-defined plugins.
fn build_registry(
    jail: Arc<PathJail>,
    project_root_dir: &Path,
    global_base: Option<&Path>,
    plugin_disabled: &[String],
    agent_disabled: &[String],
    lsp: Option<Arc<harness::lsp::LspManager>>,
    mcp_tools: Vec<Arc<dyn harness::tools::Tool>>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::project_tools(jail);
    if let Some(lsp) = lsp {
        registry = registry.with_lsp(lsp);
    }
    registry = registry.add(Arc::new(harness::memory::RememberTool::new(
        project_root_dir,
        global_base,
    )));
    let skill_roots: Vec<PathBuf> = [
        global_base.map(|b| b.join("skills")),
        Some(project_root_dir.join(".werk").join("skills")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let skills = harness::skills::discover(&skill_roots);
    if !skills.is_empty() {
        registry = registry.add(Arc::new(harness::skills::SkillTool::new(skills)));
    }
    let plugin_roots: Vec<(String, PathBuf)> = [
        global_base.map(|b| ("global".to_string(), b.join("plugins"))),
        Some(("project".to_string(), project_root_dir.join(".werk").join("plugins"))),
    ]
    .into_iter()
    .flatten()
    .collect();
    let (plugins, _) = harness::plugins::discover(&plugin_roots);
    for def in plugins {
        if plugin_disabled.iter().any(|d| d == &def.name) {
            continue;
        }
        registry = registry.add(Arc::new(harness::plugins::PluginTool::new(
            def,
            project_root_dir.to_path_buf(),
        )));
    }
    for tool in mcp_tools {
        registry = registry.add(tool);
    }
    if !agent_disabled.is_empty() {
        let names: Vec<&str> = agent_disabled.iter().map(String::as_str).collect();
        registry = registry.without(&names);
    }
    registry
}

fn global_base_dir() -> Option<PathBuf> {
    crate::config::data_dir().map(|d| d.join("werk"))
}

/// Enabled MCP servers → registered tools. Servers start on first use and
/// stay warm across runs; startup/discovery failures surface as notices.
async fn discover_mcp_tools(
    state: &AppState,
    app_config: &crate::config::AppConfig,
) -> (Vec<Arc<dyn harness::tools::Tool>>, Vec<String>) {
    let specs = match crate::mcp::agent_specs(&app_config.mcp_disabled) {
        Ok(specs) => specs,
        Err(e) => return (Vec::new(), vec![format!("MCP config: {e:#}")]),
    };
    if specs.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let manager = state.mcp_agent.manager(&specs, true);
    // Spawning npx/uvx servers can take seconds; keep it off the async pool.
    let worker = manager.clone();
    match tokio::task::spawn_blocking(move || worker.tools()).await {
        Ok((tools, errors)) => {
            // Newly discovered schemas are persisted for later runs.
            state.mcp_agent.persist_cache(&manager);
            (
                tools
                    .into_iter()
                    .map(|t| t as Arc<dyn harness::tools::Tool>)
                    .collect(),
                errors,
            )
        }
        Err(_) => (Vec::new(), vec!["MCP discovery failed".to_string()]),
    }
}

/// API key the running server was launched with, if any.
fn server_api_key(state: &AppState) -> Option<String> {
    state
        .server
        .lock()
        .unwrap()
        .config
        .as_ref()
        .and_then(|c| c.extra_params.get("api-key").cloned())
        .filter(|k| !k.trim().is_empty())
}

/// Chat client for the local server, carrying its API key when present.
fn server_client(port: u16, state: &AppState) -> LlmClient {
    LlmClient::with_key(format!("http://127.0.0.1:{port}"), server_api_key(state))
}

/// Router preset entries for the configured roles. Role ctx overrides win
/// over the launch `base_ctx`; sibling templates auto-attach unless the
/// launch pins an explicit template.
pub fn router_role_entries(
    roles: &crate::config::HarnessRoles,
    params: &crate::config::HarnessRoleParams,
    base_ctx: Option<u32>,
    explicit_template: bool,
) -> Vec<crate::server::PresetEntry> {
    let mut entries = Vec::new();
    let mut toggles = Vec::new();
    for (role, path) in [
        ("orchestrator", roles.orchestrator.clone()),
        ("worker", roles.worker.clone()),
    ] {
        let Some(path) = path else { continue };
        let params = if role == "worker" {
            &params.worker
        } else {
            &params.orchestrator
        };
        entries.push(crate::server::PresetEntry {
            path,
            ctx_size: params.ctx_size.or(base_ctx),
            n_gpu_layers: params.n_gpu_layers,
            chat_template_file: None,
            ..Default::default()
        });
        toggles.push((
            params.no_chat_template,
            params.no_mmproj,
            params.no_draft,
        ));
    }
    for (e, (no_template, no_mmproj, no_draft)) in entries.iter_mut().zip(toggles) {
        let model = Path::new(&e.path);
        if !no_template && !explicit_template && e.chat_template_file.is_none() {
            e.chat_template_file = crate::server::find_jinja_sibling(model)
                .map(|p| p.to_string_lossy().to_string());
        }
        if !no_mmproj {
            e.mmproj_path = crate::server::find_mmproj_sibling(model)
                .map(|p| p.to_string_lossy().to_string());
        }
        if !no_draft {
            if let Some((draft, kind)) = crate::server::find_spec_draft(model) {
                e.draft_model = Some(draft.to_string_lossy().to_string());
                e.spec_type = Some(
                    match kind {
                        crate::server::SpecDraftKind::Dspark => "draft-dspark",
                        crate::server::SpecDraftKind::MtpHead => "draft-mtp",
                    }
                    .to_string(),
                );
            }
        }
    }
    entries
}

/// Active project path, else the server working dir for ad-hoc runs.
pub(crate) fn project_root(state: &AppState) -> Result<PathBuf> {
    let config = state.config.lock().unwrap();
    if let Some(id) = config.harness_active_project.as_ref() {
        if let Some(p) = config.harness_projects.iter().find(|p| &p.id == id) {
            let path = PathBuf::from(&p.path);
            if path.is_dir() {
                return Ok(path);
            }
            anyhow::bail!("Project folder is gone: {}", p.path);
        }
    }
    if let Some(dir) = config.server_working_dir.clone() {
        let path = PathBuf::from(&dir);
        if path.is_dir() {
            return Ok(path);
        }
    }
    anyhow::bail!("No active project — add one first")
}

/// Orchestrator context: role override → launch n_ctx → GGUF length.
fn role_context_limit(
    role_ctx: Option<u32>,
    launch_n_ctx: u32,
    model_path: Option<&str>,
) -> Option<u64> {
    if let Some(n) = role_ctx.filter(|n| *n > 0) {
        return Some(n as u64);
    }
    if launch_n_ctx > 0 {
        return Some(launch_n_ctx as u64);
    }
    model_path.and_then(|p| {
        crate::models::read_model_metadata(Path::new(p)).and_then(|m| m.context_length)
    })
}

/// The built-in agent system prompt (before the project directory line is
/// appended). Shown in Settings so the user can see what the default is.
#[tauri::command]
#[specta::specta]
pub async fn get_harness_system_prompt_default(state: State<'_, AppState>) -> Result<String, String> {
    let (verify, server_mode) = {
        let c = state.config.lock().unwrap();
        (c.verify_mode, c.server_mode)
    };
    Ok(system_prompt_for(verify, server_mode))
}

// ── Sessions ──────────────────────────────────────────────────────────────

fn sessions_dir() -> Option<PathBuf> {
    crate::config::data_dir().map(|d| d.join("werk").join("sessions"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionFile {
    id: String,
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    created: u32,
    updated: u32,
    messages: Vec<ChatMessage>,
    #[serde(default)]
    meta: HashMap<usize, MessageMeta>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub updated: u32,
    pub message_count: u32,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct MetaEntry {
    pub index: u32,
    pub meta: MessageMeta,
}

/// Flat wire copy of a transcript message; multimodal content splits
/// into text plus image URLs.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct HistoryMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<HistoryToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct HistoryToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

fn to_history_message(m: &ChatMessage) -> HistoryMessage {
    let (content, images) = split_content(m.content.as_ref());
    HistoryMessage {
        role: m.role.clone(),
        content,
        images,
        tool_calls: m.tool_calls.as_ref().map(|calls| {
            calls
                .iter()
                .map(|c| HistoryToolCall {
                    id: c.id.clone(),
                    name: c.function.name.clone(),
                    arguments: c.function.arguments.clone(),
                })
                .collect()
        }),
        tool_call_id: m.tool_call_id.clone(),
    }
}

/// Transcript content is a string for plain turns but a parts array for
/// multimodal ones — normalize so renderers never receive an object.
fn split_content(content: Option<&serde_json::Value>) -> (Option<String>, Option<Vec<String>>) {
    let Some(content) = content else {
        return (None, None);
    };
    if let Some(text) = content.as_str() {
        return (Some(text.to_string()), None);
    }
    let Some(parts) = content.as_array() else {
        return (None, None);
    };
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for part in parts {
        let obj = match part.as_object() {
            Some(o) => o,
            None => continue,
        };
        match obj.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
                    texts.push(text.to_string());
                }
            }
            Some("image_url") => {
                if let Some(url) = obj
                    .get("image_url")
                    .and_then(|u| u.as_object())
                    .and_then(|u| u.get("url"))
                    .and_then(|u| u.as_str())
                {
                    if !url.is_empty() {
                        images.push(url.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    let content = if texts.is_empty() { None } else { Some(texts.join("\n\n")) };
    let images = if images.is_empty() { None } else { Some(images) };
    (content, images)
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct HistoryView {
    pub messages: Vec<HistoryMessage>,
    pub meta: Vec<MetaEntry>,
}

fn now_secs() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

fn session_path(id: &str) -> Option<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    sessions_dir().map(|d| d.join(format!("{id}.json")))
}

fn save_session(state: &AppState, title: Option<String>) -> Result<()> {
    let messages = state.harness.history.lock().unwrap().clone();
    save_session_messages(state, &messages, title)
}

/// Snapshot a transcript for the open session. Also called right after the
/// first send so a new chat shows up in the sessions list while it streams.
fn save_session_messages(
    state: &AppState,
    messages: &[ChatMessage],
    title: Option<String>,
) -> Result<()> {
    let (id, project) = {
        let sid = state.harness.session_id.lock().unwrap().clone();
        let project = state.config.lock().unwrap().harness_active_project.clone();
        match sid {
            Some(id) => (id, project),
            None => return Ok(()),
        }
    };
    let Some(path) = session_path(&id) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let meta = state.harness.meta.lock().unwrap().clone();
    let title = title.unwrap_or_else(|| {
        messages
            .iter()
            .find(|m| m.role == "user")
            .and_then(|m| m.content.as_ref())
            .and_then(|c| c.as_str())
            .map(|t| t.chars().take(60).collect::<String>())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Untitled".to_string())
    });
    let now = now_secs();
    let created = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<SessionFile>(&t).ok())
        .map(|s| s.created)
        .unwrap_or(now);
    let file = SessionFile {
        id,
        title,
        project,
        created,
        updated: now,
        messages: messages.to_vec(),
        meta,
    };
    std::fs::write(&path, serde_json::to_string_pretty(&file)?)?;
    Ok(())
}

fn truncate_meta(state: &AppState, len: usize) {
    state.harness.meta.lock().unwrap().retain(|&i, _| i < len);
}

/// Files the agent wrote or edited since the last user turn, compared against
/// `base` (the HEAD captured when the run started) so committed work counts.
fn last_run_changes(
    state: &AppState,
    root: &std::path::Path,
    base: Option<&str>,
) -> Option<RunChanges> {
    let touched = touched_since_last_user(state, root);
    if touched.is_empty() {
        return None;
    }
    let names = git_name_status(root, base);
    let numstat = git_numstat_map(root, base);
    let statuses = git_status_map(root).unwrap_or_default();
    let mut files = Vec::new();
    let (mut added, mut removed) = (0u32, 0u32);
    for path in touched {
        let untracked = statuses.get(&path).map(|s| s == "??").unwrap_or(false);
        let code = names.get(&path).map(String::as_str);
        let status = if untracked || code == Some("A") {
            "added"
        } else if code == Some("D") || !root.join(&path).exists() {
            "deleted"
        } else if code.is_some() || statuses.contains_key(&path) {
            "modified"
        } else {
            continue;
        };
        let (a, r) = if status == "added" {
            (count_file_lines(&root.join(&path)), 0)
        } else {
            numstat.get(&path).copied().unwrap_or((0, 0))
        };
        added += a;
        removed += r;
        files.push(RunChange {
            path,
            status: status.to_string(),
            added: a,
            removed: r,
        });
    }
    if files.is_empty() {
        return None;
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Some(RunChanges { files, added, removed })
}

/// Write/edit tool paths from the transcript after the last user message.
fn touched_since_last_user(state: &AppState, root: &std::path::Path) -> Vec<String> {
    let history = state.harness.history.lock().unwrap();
    let mut out: Vec<String> = Vec::new();
    for msg in history.iter().rev() {
        if msg.role == "user" {
            break;
        }
        let Some(calls) = msg.tool_calls.as_ref() else {
            continue;
        };
        for call in calls {
            if call.function.name != "write_file" && call.function.name != "edit_file" {
                continue;
            }
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.function.arguments) {
                if let Some(path) = args.get("path").and_then(|p| p.as_str()) {
                    let norm = path.replace('\\', "/");
                    let root_norm = root.to_string_lossy().replace('\\', "/");
                    let rel = norm
                        .strip_prefix(&format!("{root_norm}/"))
                        .unwrap_or(&norm)
                        .trim_start_matches('/')
                        .to_string();
                    if !rel.is_empty() {
                        out.push(rel);
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `git status --porcelain` as path -> XY code (None when not a repo).
fn git_status_map(root: &std::path::Path) -> Option<HashMap<String, String>> {
    if !root.join(".git").exists() {
        return None;
    }
    let out = crate::hidden::command("git")
        .args(["-C"])
        .arg(root)
        .args(["status", "--porcelain", "--no-renames"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map = HashMap::new();
    for line in text.lines() {
        if line.len() < 4 {
            continue;
        }
        let (code, rest) = line.split_at(2);
        let path = rest.trim_start();
        let path = path.trim_matches('"').replace("\\\"", "\"");
        map.insert(path, code.trim().to_string());
    }
    Some(map)
}

/// Current HEAD sha, for comparing a run's changes against its start.
fn git_head(root: &std::path::Path) -> Option<String> {
    let out = crate::hidden::command("git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// `git diff <base|HEAD> --name-status` as path -> A/M/D/T.
fn git_name_status(root: &std::path::Path, base: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let target = base.unwrap_or("HEAD");
    let Ok(out) = crate::hidden::command("git")
        .args(["-C"])
        .arg(root)
        .args(["diff", target, "--name-status", "--no-renames"])
        .output()
    else {
        return map;
    };
    if !out.status.success() {
        return map;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut parts = line.split('\t');
        let (Some(code), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        let path = path.trim_matches('"').replace("\\\"", "\"");
        map.insert(path, code.trim().chars().next().unwrap_or('M').to_string());
    }
    map
}

/// `git diff <base|HEAD> --numstat` as path -> (added, removed) lines.
fn git_numstat_map(root: &std::path::Path, base: Option<&str>) -> HashMap<String, (u32, u32)> {
    let mut map = HashMap::new();
    let target = base.unwrap_or("HEAD");
    let Ok(out) = crate::hidden::command("git")
        .args(["-C"])
        .arg(root)
        .args(["diff", target, "--numstat", "--no-renames"])
        .output()
    else {
        return map;
    };
    if !out.status.success() {
        return map;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut parts = line.split('\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = path.trim_matches('"').replace("\\\"", "\"");
        map.insert(
            path,
            (a.parse().unwrap_or(0), r.parse().unwrap_or(0)),
        );
    }
    map
}

/// Line count for a new (untracked) file; 0 when unreadable or huge.
fn count_file_lines(path: &std::path::Path) -> u32 {
    const CAP: u64 = 4 * 1024 * 1024;
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if meta.len() > CAP {
        return 0;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    if text.is_empty() {
        return 0;
    }
    let newlines = text.bytes().filter(|b| *b == b'\n').count() as u32;
    if text.ends_with('\n') {
        newlines
    } else {
        newlines + 1
    }
}

/// Reindex footer metadata after compaction cuts: each cut removed
/// `history[1..cut]` and inserted one summary. Entries inside a removed
/// span are dropped; the system message (index 0) always survives.
fn shift_meta_for_compaction(state: &AppState, cuts: &[usize]) {
    if cuts.is_empty() {
        return;
    }
    let mut meta = state.harness.meta.lock().unwrap();
    let mut shifted = HashMap::new();
    for (index, entry) in meta.drain() {
        if index == 0 {
            shifted.insert(0, entry);
            continue;
        }
        let mut adjusted = index;
        let mut dropped = false;
        for cut in cuts {
            if adjusted < *cut {
                dropped = true;
                break;
            }
            adjusted = adjusted - cut + 2;
        }
        if !dropped {
            shifted.insert(adjusted, entry);
        }
    }
    *meta = shifted;
}

struct RunningGuard(Arc<HarnessRuntime>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
    }
}

/// A user-picked file: images ride as data URLs, text inlines.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SendAttachment {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AttachmentRead {
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

fn is_image_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["png", "jpg", "jpeg", "webp", "gif", "bmp"]
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// Read a user-picked file for sending: images as base64 (15 MB cap),
/// anything else as text (200 KB cap).
#[tauri::command]
#[specta::specta]
pub async fn harness_read_attachment(path: String) -> Result<AttachmentRead, String> {
    let file = PathBuf::from(&path);
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "Not a file".to_string())?
        .to_string();
    if !file.is_file() {
        return Err(format!("Not a file: {path}"));
    }
    if is_image_name(&name) {
        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
        if bytes.len() > 15 * 1024 * 1024 {
            return Err(format!("{name} is larger than 15 MB"));
        }
        return Ok(AttachmentRead {
            kind: "image".to_string(),
            name,
            data_base64: Some(base64_encode(&bytes)),
            text: None,
        });
    }
    let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let text: String = text.chars().take(200_000).collect();
    Ok(AttachmentRead { kind: "text".into(), name, data_base64: None, text: Some(text) })
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | (*chunk.get(2).unwrap_or(&0) as u32);
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Fold attachments into a user message: images as content parts, text as
/// fenced blocks. Returns (message, image data URLs for the transcript).
fn with_attachments(message: &str, attachments: &[SendAttachment]) -> (ChatMessage, Vec<String>) {
    let mut text = message.to_string();
    let mut parts = Vec::new();
    let mut images = Vec::new();
    for a in attachments {
        if a.kind == "image" {
            if let Some(data) = a.data_base64.as_deref().filter(|d| !d.is_empty()) {
                let mime = if a.name.to_lowercase().ends_with(".jpg") || a.name.to_lowercase().ends_with(".jpeg") {
                    "image/jpeg"
                } else if a.name.to_lowercase().ends_with(".webp") {
                    "image/webp"
                } else {
                    "image/png"
                };
                let url = format!("data:{mime};base64,{data}");
                images.push(url.clone());
                parts.push(serde_json::json!({"type": "image_url", "image_url": {"url": url}}));
            }
        } else if let Some(body) = a.text.as_deref() {
            text.push_str(&format!("\n\n[Attachment: {}]\n{body}", a.name));
        }
    }
    if parts.is_empty() {
        (ChatMessage::user(text), images)
    } else {
        let mut all = vec![serde_json::json!({"type": "text", "text": text})];
        all.extend(parts);
        (
            ChatMessage {
                role: "user".into(),
                content: Some(serde_json::Value::Array(all)),
                tool_calls: None,
                tool_call_id: None,
            },
            images,
        )
    }
}

// ── Commands ──────────────────────────────────────────────────────────────

/// Send a message to the agent; streams over `harness_event`.
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_send(
    message: String,
    reasoning_effort: Option<String>,
    attachments: Option<Vec<SendAttachment>>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RunResult, String> {
    if state
        .harness
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("An agent run is already in progress".to_string());
    }
    let _running_guard = RunningGuard(state.harness.clone());
    // A stale Stop from the previous run must not kill this one.
    state.harness_abort.store(false, Ordering::SeqCst);
    state.harness.steering.lock().unwrap().clear();

    load_permission_grants(&state);
    let root = project_root(&state).map_err(|e| e.to_string())?;
    let jail = Arc::new(
        PathJail::new(&root, &[]).map_err(|e| e.to_string())?,
    );
    let global_base = global_base_dir();
    let app_config = state.config.lock().unwrap().clone();
    let disabled = app_config.plugin_disabled.clone();
    let lsp = app_config.lsp_enabled.then(|| state.lsp.clone());
    // MCP tools run in the harness (all modes): spawn servers once, reuse the
    // manager across runs; servers are gated by per-tool approval.
    let (mcp_tools, mcp_errors) = discover_mcp_tools(&state, &app_config).await;
    for text in mcp_errors {
        let _ = app.emit(
            "harness_event",
            serde_json::json!({"type": "notice", "text": text}),
        );
    }
    let registry = build_registry(
        jail.clone(),
        &root,
        global_base.as_deref(),
        &disabled,
        &app_config.agent_tools_disabled,
        lsp,
        mcp_tools,
    );

    // Roles only apply in router mode; single mode chats with the loaded
    // model, external mode with the configured provider target.
    let mode = app_config.server_mode;
    let router = mode == crate::config::ServerMode::Router;
    let worker_path = if router {
        app_config
            .harness_roles
            .worker
            .clone()
            .filter(|p| !p.trim().is_empty())
    } else {
        None
    };
    let orchestrator_id = if router {
        app_config
            .harness_roles
            .orchestrator
            .clone()
            .filter(|p| !p.trim().is_empty())
    } else if mode == crate::config::ServerMode::External {
        app_config
            .external_target
            .clone()
            .filter(|p| !p.trim().is_empty())
    } else {
        None
    };
    let max_turns = app_config.harness_max_turns.max(1) as usize;
    let subagent_max_turns = app_config.harness_subagent_max_turns.max(1) as usize;
    let verify_mode = app_config.verify_mode;

    // Router ids are the preset section names (file stems, deduped), never
    // the filesystem paths the roles store.
    let router_ids = if router {
        crate::server::router_model_names(&router_role_entries(
            &app_config.harness_roles,
            &app_config.harness_role_params,
            None,
            false,
        ))
    } else {
        std::collections::HashMap::new()
    };
    let model_id = |path: &str| {
        router_ids
            .get(path)
            .cloned()
            .unwrap_or_else(|| crate::server::file_stem_or_self(path))
    };

    // History: resume the open session, then ensure one system prompt.
    if state.harness.history.lock().unwrap().is_empty() {
        if let Some(id) = state.harness.session_id.lock().unwrap().clone() {
            if let Some(path) = session_path(&id) {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(saved) = serde_json::from_str::<SessionFile>(&text) {
                        *state.harness.history.lock().unwrap() = saved.messages;
                        *state.harness.meta.lock().unwrap() = saved.meta;
                    }
                }
            }
        }
    }
    let mut history = std::mem::take(&mut *state.harness.history.lock().unwrap());
    if !history.iter().any(|m| m.role == "system") {
        let base = {
            let c = state.config.lock().unwrap();
            let verify = c.verify_mode;
            let server_mode = c.server_mode;
            c.harness_system_prompt
                .clone()
                .filter(|p| !p.trim().is_empty())
                .unwrap_or_else(move || system_prompt_for(verify, server_mode))
        };
        let memory = harness::memory::load_block(
            global_base.as_deref().map(|b| b.join("MEMORY.md")).as_deref(),
            &root,
        );
        history.insert(
            0,
            ChatMessage::system(format!(
                "{base}\n\nProject directory: {}{memory}",
                root.display()
            )),
        );
    }
    let (user_msg, _attached_images) = with_attachments(&message, &attachments.unwrap_or_default());
    history.push(user_msg);
    if state.harness.session_id.lock().unwrap().is_none() {        let base = format!("s{}", now_secs());
        let mut id = base.clone();
        let mut n = 2;
        while session_path(&id).map(|p| p.exists()).unwrap_or(false) {
            id = format!("{base}-{n}");
            n += 1;
        }
        *state.harness.session_id.lock().unwrap() = Some(id);
    }
    // Persist the sent transcript right away so the new chat appears in the
    // sessions list while the first reply streams (and survives a crash).
    if let Err(e) = save_session_messages(&state, &history, None) {
        let _ = app.emit(
            "harness_event",
            serde_json::json!({"type": "notice", "text": format!("Session save failed: {e}")}),
        );
    }

    // External API mode resolves the configured provider target; local modes
    // never mix in external endpoints.
    let orch_target = if mode == crate::config::ServerMode::External {
        let target = orchestrator_id.as_deref().ok_or_else(|| {
            "External API mode: no model selected — pick a provider and model on the Mode page"
                .to_string()
        })?;
        let (p, m) = crate::config::Provider::split_target(target, &app_config.providers)
            .ok_or_else(|| {
                format!(
                    "External API mode: \"{target}\" has no configured provider (check the Mode tab)"
                )
            })?;
        Some((p.clone(), m.to_string()))
    } else {
        None
    };

    // One HTTP client per external provider in use, built before borrowing.
    let mut external: Vec<(String, LlmClient)> = Vec::new();
    for (provider, _) in orch_target.iter() {
        if !external.iter().any(|(id, _)| id == &provider.id) {
            external.push((
                provider.id.clone(),
                LlmClient::with_key(
                    provider.base_url.trim_end_matches('/').to_string(),
                    provider.api_key.clone(),
                ),
            ));
        }
    }
    let external_client = |provider: &crate::config::Provider| -> &LlmClient {
        &external
            .iter()
            .find(|(id, _)| id == &provider.id)
            .expect("provider client pre-created")
            .1
    };

    let launch_n_ctx = state
        .server
        .lock()
        .unwrap()
        .config
        .as_ref()
        .map(|c| c.n_ctx)
        .unwrap_or(0);
    let running_port = match state.server.lock().unwrap().status.clone() {
        crate::server::ServerStatus::Running { port, .. } => Some(port),
        _ => None,
    };
    // A local worker still needs the local server.
    if worker_path.is_some() && running_port.is_none() {
        return Err(
            "The worker role is a local model but the server is not running — start it on the \
             Run page first."
                .to_string(),
        );
    }

    let local_client;
    let (client, model, context_limit): (&LlmClient, Option<String>, Option<u64>) =
        if let Some((provider, model_id)) = &orch_target {
            (
                external_client(provider),
                Some(model_id.clone()),
                provider.context_length.map(|c| c as u64),
            )
        } else {
            let Some(port) = running_port else {
                return Err("Server is not running".to_string());
            };
            local_client = server_client(port, &state);
            let orch_path = orchestrator_id.clone().or_else(|| {
                state
                    .server
                    .lock()
                    .unwrap()
                    .config
                    .as_ref()
                    .map(|c| c.model_path.clone())
                    .filter(|p| !p.is_empty())
            });
            (
                &local_client,
                orchestrator_id.clone().map(|p| model_id(&p)),
                role_context_limit(
                    app_config.harness_role_params.orchestrator.ctx_size,
                    launch_n_ctx,
                    orch_path.as_deref(),
                ),
            )
        };

    let (worker_client, worker_model, worker_limit) = (
        None,
        worker_path.clone().map(|p| model_id(&p)),
        worker_path.as_deref().and_then(|w| {
            role_context_limit(
                app_config.harness_role_params.worker.ctx_size,
                launch_n_ctx,
                Some(w),
            )
        }),
    );

    let gate = Arc::new(UiGate { app: app.clone(), runtime: state.harness.clone() });
    let steer_rt = state.harness.clone();
    let steer: Arc<dyn Fn() -> Vec<String> + Send + Sync> =
        Arc::new(move || std::mem::take(&mut *steer_rt.steering.lock().unwrap()));

    let mut last_usage: Option<(u32, u32)> = None;
    let mut sink = |ev: StreamEvent| {
        if let StreamEvent::Usage { prompt_tokens, completion_tokens } = &ev {
            last_usage = Some((*prompt_tokens as u32, *completion_tokens as u32));
        }
        let _ = app.emit("harness_event", serde_json::to_value(&ev).unwrap_or_default());
    };
    let app_events = app.clone();
    let mut event_sink = move |ev: AgentEvent| {
        let _ = app_events.emit("harness_event", serde_json::to_value(&ev).unwrap_or_default());
    };

    let run = AgentRun {
        client,
        registry: Arc::new(registry),
        engine: state.harness.engine.clone(),
        model: model.clone(),
        project: state.config.lock().unwrap().harness_active_project.clone(),
        reasoning_effort: reasoning_effort.filter(|e| !e.is_empty()),
        max_turns,
        context_limit,
        verify_mode,
        compactions_log: Arc::new(Mutex::new(Vec::new())),
        subagents: Some(harness::agent::Subagents {
            jail,
            max_turns: subagent_max_turns,
            model: worker_model,
            exec_enabled: true,
            context_limit: worker_limit,
            client: worker_client,
        }),
    };

    let should_stop = state.harness_abort.clone();
    let run_started = std::time::Instant::now();
    let base_head = git_head(&root);
    let result = run
        .run(
            &mut history,
            Arc::new(move || should_stop.load(Ordering::SeqCst)) as Arc<dyn Fn() -> bool + Send + Sync>,
            gate,
            steer,
            &mut sink,
            &mut event_sink,
        )
        .await;

    *state.harness.history.lock().unwrap() = history;
    shift_meta_for_compaction(&state, &run.compactions_log.lock().unwrap().clone());
    // A failed run still persists its transcript before surfacing the error.
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(e) => {
            if let Err(e) = save_session(&state, None) {
                let _ = app.emit(
                    "harness_event",
                    serde_json::json!({"type": "notice", "text": format!("Session save failed: {e}")}),
                );
            }
            return Err(e.to_string());
        }
    };
    let reasoning = if outcome.reasoning.is_empty() {
        None
    } else {
        Some(outcome.reasoning.clone())
    };
    // The model that actually served the request: the resolved role target,
    // else the loaded single model.
    let served = state
        .server
        .lock()
        .unwrap()
        .config
        .as_ref()
        .map(|c| c.model_path.clone())
        .filter(|p| !p.is_empty());
    let model = model.or(served).map(|p| model_label(&p));
    let footer_index = state
        .harness
        .history
        .lock()
        .unwrap()
        .iter()
        .rposition(|m| m.role == "assistant");
    let changes = last_run_changes(&state, &root, base_head.as_deref());
    if let Some(index) = footer_index {
        // Elapsed covers the whole run (prompt, every turn, tool calls), not
        // just the last generation the way the model outcome reports it.
        let total_ms = run_started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        state.harness.meta.lock().unwrap().insert(
            index,
            MessageMeta {
                model: model.clone(),
                tokens_per_sec: outcome.tokens_per_sec,
                gen_tokens: Some(outcome.gen_tokens as u32),
                prompt_tokens: outcome.prompt_tokens.map(|p| p as u32).or(last_usage.map(|(p, _)| p)),
                elapsed_ms: Some(total_ms),
                reasoning: reasoning.clone(),
                changes: changes.clone(),
            },
        );
        // One save with the final transcript and footer metadata.
        if let Err(e) = save_session(&state, None) {
            let _ = app.emit(
                "harness_event",
                serde_json::json!({"type": "notice", "text": format!("Session save failed: {e}")}),
            );
        }
    }
    Ok(RunResult {
        text: outcome.text,
        model,
        tokens_per_sec: outcome.tokens_per_sec,
        gen_tokens: outcome.gen_tokens as u32,
        prompt_tokens: outcome.prompt_tokens.map(|p| p as u32).or(last_usage.map(|(p, _)| p)),
        elapsed_ms: run_started.elapsed().as_millis().min(u32::MAX as u128) as u32,
        reasoning,
        changes,
    })
}

/// Stop the running agent loop.
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_abort(state: State<'_, AppState>) -> Result<(), String> {
    state.harness_abort.store(true, Ordering::SeqCst);
    if let Some(tx) = state.harness.pending.lock().unwrap().take() {
        let _ = tx.send(Approved::Denied);
    }
    if let Some(tx) = state.harness.pending_question.lock().unwrap().take() {
        let _ = tx.send(QuestionAnswer::cancelled());
    }
    state.harness.steering.lock().unwrap().clear();
    Ok(())
}

/// Queue a user message into the running loop (next turn boundary).
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_steer(
    message: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let text = message.trim().to_string();
    if text.is_empty() {
        return Err("Empty message".to_string());
    }
    if text.chars().count() > 8_000 {
        return Err("Steer message too long (8000 chars max)".to_string());
    }
    if !state.harness.running.load(Ordering::SeqCst) {
        return Err("No run in progress".to_string());
    }
    state.harness.steering.lock().unwrap().push(text);
    Ok(())
}

/// Clear the live transcript (keeps saved sessions).
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_reset(state: State<'_, AppState>) -> Result<(), String> {
    state.harness.history.lock().unwrap().clear();
    truncate_meta(&state, 0);
    *state.harness.session_id.lock().unwrap() = None;
    if let Some(tx) = state.harness.pending.lock().unwrap().take() {
        let _ = tx.send(Approved::Denied);
    }
    if let Some(tx) = state.harness.pending_question.lock().unwrap().take() {
        let _ = tx.send(QuestionAnswer::cancelled());
    }
    state.harness.steering.lock().unwrap().clear();
    Ok(())
}

/// Drop the last assistant response and the user turn that produced it.
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_rewind(state: State<'_, AppState>) -> Result<(), String> {
    if state.harness.running.load(Ordering::SeqCst) {
        return Err("Stop the running agent first".to_string());
    }
    let idx = {
        let mut history = state.harness.history.lock().unwrap();
        let Some(idx) = history.iter().rposition(|m| m.role == "user") else {
            return Err("Nothing to rewind".to_string());
        };
        history.truncate(idx);
        idx
    };
    truncate_meta(&state, idx);
    save_session(&state, None).map_err(|e| e.to_string())
}

/// Force-summarize older turns now; returns the folded message count.
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_compact(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<u32, String> {
    if state.harness.running.load(Ordering::SeqCst) {
        return Err("Stop the running agent first".to_string());
    }
    let port = match state.server.lock().unwrap().status.clone() {
        crate::server::ServerStatus::Running { port, .. } => port,
        _ => return Err("Server is not running".to_string()),
    };
    let (model, context_limit) = {
        let config = state.config.lock().unwrap();
        let model = config.harness_roles.orchestrator.clone().filter(|s| !s.is_empty());
        let launch_n_ctx = state
            .server
            .lock()
            .unwrap()
            .config
            .as_ref()
            .map(|c| c.n_ctx)
            .unwrap_or(0);
        let limit = role_context_limit(
            config.harness_role_params.orchestrator.ctx_size,
            launch_n_ctx,
            model.as_deref(),
        );
        (model, limit)
    };
    let Some(limit) = context_limit else {
        return Err("No context limit known — start the server first".to_string());
    };
    let client = server_client(port, &state);
    let app_events = app.clone();
    let mut on_event = move |ev: AgentEvent| {
        let _ = app_events.emit("harness_event", serde_json::to_value(&ev).unwrap_or_default());
    };
    let mut history = std::mem::take(&mut *state.harness.history.lock().unwrap());
    let result = harness::compact::compact_history(
        &client,
        model.as_deref(),
        &mut history,
        limit,
        true,
        &|| state.harness_abort.load(Ordering::SeqCst),
        &mut on_event,
    )
    .await;
    *state.harness.history.lock().unwrap() = history;
    let info = result.map_err(|e| e.to_string())?.unwrap_or(harness::compact::CompactionInfo {
        cut: None,
        removed: 0,
    });
    if let Some(cut) = info.cut {
        shift_meta_for_compaction(&state, &[cut]);
    }
    save_session(&state, None).map_err(|e| e.to_string())?;
    Ok(info.removed as u32)
}

/// Model badges for the input row (orchestrator model wins).
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct HarnessCapabilities {
    pub vision: bool,
    pub reasoning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

fn active_model_path(state: &AppState) -> Option<String> {
    let config = state.config.lock().unwrap();
    if config.server_mode == crate::config::ServerMode::External {
        return None;
    }
    if let Some(o) = config.harness_roles.orchestrator.clone() {
        if !o.is_empty() {
            return Some(o);
        }
    }
    if let Some(s) = config.selected_model.clone() {
        if !s.is_empty() {
            return Some(s);
        }
    }
    drop(config);
    state
        .server
        .lock()
        .unwrap()
        .config
        .as_ref()
        .map(|c| c.model_path.clone())
        .filter(|p| !p.is_empty())
}

#[tauri::command]
#[specta::specta]
pub async fn harness_agent_capabilities(
    state: State<'_, AppState>,
) -> Result<HarnessCapabilities, String> {
    let none = HarnessCapabilities { vision: false, reasoning: false, context_length: None };
    let Some(path) = active_model_path(&state) else {
        return Ok(none);
    };
    let dirs = state.config.lock().unwrap().all_model_dirs();
    let found = crate::models::list_installed_models(&dirs).into_iter().find(|m| m.path == path);
    Ok(match found {
        Some(m) => HarnessCapabilities {
            vision: m.is_vision,
            reasoning: m.is_reasoning,
            context_length: m.context_length.map(|c| c.min(u32::MAX as u64) as u32),
        },
        None => none,
    })
}

/// Reasoning effort levels from the active model's chat template.
/// Others 500 — hidden for non-reasoning models.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ReasoningOptions {
    pub supported: bool,
    pub levels: Vec<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn harness_reasoning_options(
    state: State<'_, AppState>,
) -> Result<ReasoningOptions, String> {
    let none = ReasoningOptions { supported: false, levels: Vec::new() };
    let Some(path) = active_model_path(&state) else {
        return Ok(none);
    };
    let meta = crate::models::read_model_metadata(Path::new(&path));
    let Some(meta) = meta else {
        return Ok(none);
    };
    let levels = crate::models::parse_reasoning_effort_levels(meta.chat_template.as_deref().unwrap_or(""));
    let template = meta.chat_template.as_deref().unwrap_or("");
    let supported = template.to_lowercase().contains("<think>")
        || crate::models::template_drives_reasoning(Some(template))
        || !levels.is_empty();
    Ok(ReasoningOptions { supported, levels })
}

/// Live context gauge: slot usage first, GGUF length as fallback.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ContextStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
}

fn gguf_context(path: &str) -> Option<u32> {
    crate::models::read_model_metadata(Path::new(path))
        .and_then(|m| m.context_length)
        .map(|c| c.min(u32::MAX as u64) as u32)
}

/// Effective context for the orchestrator with no live slot: role override in
/// router mode, the external provider declaration, then the local GGUF header.
fn orchestrator_context(state: &AppState) -> Option<u32> {
    let (mode, role_ctx, role_path, external) = {
        let config = state.config.lock().unwrap();
        (
            config.server_mode,
            config.harness_role_params.orchestrator.ctx_size.filter(|c| *c > 0),
            config.harness_roles.orchestrator.clone(),
            config.external_target.clone(),
        )
    };
    match mode {
        crate::config::ServerMode::External => {
            let target = external?;
            let config = state.config.lock().unwrap();
            crate::config::Provider::split_target(&target, &config.providers)
                .and_then(|(p, _)| p.context_length)
        }
        crate::config::ServerMode::Router => role_ctx
            .or_else(|| role_path.as_deref().and_then(gguf_context))
            .or_else(|| active_model_path(state).as_deref().and_then(gguf_context)),
        crate::config::ServerMode::Single => {
            active_model_path(state).as_deref().and_then(gguf_context)
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn harness_context_stats(
    state: State<'_, AppState>,
) -> Result<ContextStats, String> {
    let used = harness::compact::estimate_tokens(&state.harness.history.lock().unwrap());
    let mut stats = ContextStats {
        used: Some(used.min(u32::MAX as u64) as u32),
        total: None,
    };
    // External API mode never reads the local server, even if one runs.
    let external =
        state.config.lock().unwrap().server_mode == crate::config::ServerMode::External;
    // Take the status by value: holding the guard in the match arms would
    // deadlock orchestrator_context, which locks the same mutex.
    let status = state.server.lock().unwrap().status.clone();
    let port = match status {
        crate::server::ServerStatus::Running { port, .. } if !external => port,
        _ => {
            stats.total = orchestrator_context(&state);
            return Ok(stats);
        }
    };
    let client = server_client(port, &state);
    // Live slot sizes reflect the effective --ctx-size/--fit; the GGUF
    // header is only the training maximum.
    if let Ok(Some((ctx, prompt))) = client.slot_fill().await {
        if ctx > 0 {
            stats.total = Some(ctx.min(u32::MAX as u64) as u32);
        }
        if prompt > 0 {
            stats.used = Some(prompt.min(u32::MAX as u64) as u32);
        }
    }
    if stats.total.is_none() {
        if let Ok(Some(n)) = client.props_context().await {
            if n > 0 {
                stats.total = Some(n.min(u32::MAX as u64) as u32);
            }
        }
    }
    if stats.total.is_none() {
        if let Some(path) = active_model_path(&state) {
            stats.total = gguf_context(&path);
        }
    }
    Ok(stats)
}

/// Current transcript for rebuilding the view (sessions, restarts).
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_history(state: State<'_, AppState>) -> Result<HistoryView, String> {
    let messages = state.harness.history.lock().unwrap().clone();
    let meta = state
        .harness
        .meta
        .lock()
        .unwrap()
        .iter()
        .map(|(index, meta)| MetaEntry { index: *index as u32, meta: meta.clone() })
        .collect();
    Ok(HistoryView {
        messages: messages.iter().map(to_history_message).collect(),
        meta,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn harness_agent_decide(
    grant: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let sender = state.harness.pending.lock().unwrap().take();
    let scope = match grant.as_deref() {
        Some("once") => Approved::Once,
        Some("session") => Approved::Session,
        Some("project") => Approved::Project,
        Some("global") => Approved::Global,
        _ => Approved::Denied,
    };
    if let Some(tx) = sender {
        let _ = tx.send(scope);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn harness_question_answer(
    answer: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let sender = state.harness.pending_question.lock().unwrap().take();
    let answer = answer.trim().to_string();
    if let Some(tx) = sender {
        let _ = tx.send(if answer.is_empty() {
            QuestionAnswer::cancelled()
        } else {
            QuestionAnswer { answer, cancelled: false }
        });
    }
    Ok(())
}

// ── Sessions ──────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn harness_sessions_list() -> Result<Vec<SessionSummary>, String> {
    let Some(dir) = sessions_dir() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(file) = serde_json::from_str::<SessionFile>(&text) {
                    out.push(SessionSummary {
                        id: file.id,
                        title: file.title,
                        project: file.project,
                        updated: file.updated,
                        message_count: file.messages.len() as u32,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| b.updated.cmp(&a.updated));
    Ok(out)
}

#[tauri::command]
#[specta::specta]
pub async fn harness_session_load(
    id: String,
    state: State<'_, AppState>,
) -> Result<HistoryView, String> {
    let Some(path) = session_path(&id) else {
        return Err("Unknown session".to_string());
    };
    let text = std::fs::read_to_string(&path).map_err(|_| "Session file is corrupt".to_string())?;
    let file: SessionFile =
        serde_json::from_str(&text).map_err(|_| "Session file is corrupt".to_string())?;
    *state.harness.history.lock().unwrap() = file.messages.clone();
    *state.harness.meta.lock().unwrap() = file.meta.clone();
    *state.harness.session_id.lock().unwrap() = Some(file.id.clone());
    Ok(HistoryView {
        messages: file.messages.iter().map(to_history_message).collect(),
        meta: file
            .meta
            .into_iter()
            .map(|(index, meta)| MetaEntry { index: index as u32, meta })
            .collect(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn harness_session_delete(
    id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.harness.session_id.lock().unwrap().as_deref() == Some(&id) {
        state.harness.history.lock().unwrap().clear();
        truncate_meta(&state, 0);
        *state.harness.session_id.lock().unwrap() = None;
    }
    if let Some(path) = session_path(&id) {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn read_session(id: &str) -> Result<SessionFile, String> {
    let path = session_path(id).ok_or_else(|| "Unknown session".to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|_| "Session file is corrupt".to_string())?;
    serde_json::from_str(&text).map_err(|_| "Session file is corrupt".to_string())
}

fn write_session(file: &SessionFile) -> Result<(), String> {
    let path = session_path(&file.id).ok_or_else(|| "Unknown session".to_string())?;
    let content = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn harness_session_rename(id: String, title: String) -> Result<(), String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Title is empty".to_string());
    }
    let mut file = read_session(&id)?;
    file.title = title;
    file.updated = now_secs();
    write_session(&file)
}

/// Write the session transcript JSON to a user-picked path.
#[tauri::command]
#[specta::specta]
pub async fn harness_session_export(id: String, path: String) -> Result<(), String> {
    let file = read_session(&id)?;
    let dest = PathBuf::from(&path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&dest, content).map_err(|e| e.to_string())
}

// ── Projects ──────────────────────────────────────────────────────────────

fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    slug.chars().take(48).collect()
}

#[tauri::command]
#[specta::specta]
pub async fn harness_project_add(
    path: String,
    state: State<'_, AppState>,
) -> Result<HarnessProject, String> {
    let dir = PathBuf::from(path.trim());
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", dir.display()));
    }
    let mut config = state.config.lock().unwrap();
    if let Some(existing) = config
        .harness_projects
        .iter()
        .find(|p| PathBuf::from(&p.path).as_os_str() == dir.as_os_str())
        .map(|p| (p.id.clone(), p.clone()))
    {
        config.harness_active_project = Some(existing.0);
        config.save().map_err(|e| e.to_string())?;
        return Ok(existing.1);
    }
    let stem = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let mut id = slugify(stem);
    if id.is_empty() {
        id = format!("p{}", now_secs());
    }
    let mut n = 2;
    while config.harness_projects.iter().any(|p| p.id == id) {
        id = format!("{}-{n}", slugify(stem));
        n += 1;
    }
    let project = HarnessProject {
        id: id.clone(),
        name: stem.to_string(),
        path: dir.to_string_lossy().to_string(),
        created: now_secs(),
        extra_read: Vec::new(),
    };
    config.harness_projects.push(project.clone());
    config.harness_active_project = Some(id);
    config.save().map_err(|e| e.to_string())?;
    Ok(project)
}

#[tauri::command]
#[specta::specta]
pub async fn harness_project_remove(
    id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.harness_projects.retain(|p| p.id != id);
    if config.harness_active_project.as_deref() == Some(&id) {
        config.harness_active_project = None;
        state.harness.history.lock().unwrap().clear();
        truncate_meta(&state, 0);
        *state.harness.session_id.lock().unwrap() = None;
    }
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn harness_project_set_active(
    id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    {
        let mut config = state.config.lock().unwrap();
        if let Some(ref wanted) = id {
            if !config.harness_projects.iter().any(|p| &p.id == wanted) {
                return Err("Unknown project".to_string());
            }
        }
        config.harness_active_project = id;
        config.save().map_err(|e| e.to_string())?;
    }
    state.harness.history.lock().unwrap().clear();
    truncate_meta(&state, 0);
    *state.harness.session_id.lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn harness_project_rename(
    id: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Name is empty".to_string());
    }
    let mut config = state.config.lock().unwrap();
    let Some(p) = config.harness_projects.iter_mut().find(|p| p.id == id) else {
        return Err("Unknown project".to_string());
    };
    p.name = name;
    config.save().map_err(|e| e.to_string())
}

/// Read-only allowlist for the active project (applies on next run).
#[tauri::command]
#[specta::specta]
pub async fn set_harness_project_extra_read(
    paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    let Some(active) = config.harness_active_project.clone() else {
        return Err("No active project".to_string());
    };
    let Some(p) = config.harness_projects.iter_mut().find(|p| p.id == active) else {
        return Err("Unknown project".to_string());
    };
    let mut seen = std::collections::HashSet::new();
    p.extra_read = paths
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .collect();
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn harness_git_is_repo(root: String) -> Result<bool, String> {
    Ok(crate::worktree::is_repo(Path::new(&root)))
}

#[tauri::command]
#[specta::specta]
pub async fn harness_worktree_list(
    root: String,
) -> Result<Vec<crate::worktree::WorktreeDto>, String> {
    crate::worktree::list(Path::new(&root)).map_err(|e| e.to_string())
}

/// Create a branch worktree in a sibling folder; returns its path.
#[tauri::command]
#[specta::specta]
pub async fn harness_worktree_add(root: String, branch: String) -> Result<String, String> {
    crate::worktree::add(Path::new(&root), &branch)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn harness_worktree_remove(
    root: String,
    path: String,
    force: bool,
) -> Result<(), String> {
    crate::worktree::remove(Path::new(&root), &path, force).map_err(|e| e.to_string())
}

// ── Agent settings ────────────────────────────────────────────────────────

/// Footer label for a model path: just the file name.
fn model_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_harness_roles(
    orchestrator: Option<String>,
    worker: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.harness_roles.orchestrator = orchestrator.filter(|p| !p.trim().is_empty());
    config.harness_roles.worker = worker.filter(|p| !p.trim().is_empty());
    config.save().map_err(|e| e.to_string())
}

/// Per-role server overrides; None inherits the default.
#[tauri::command]
#[specta::specta]
pub async fn set_role_params(
    role: String,
    ctx_size: Option<u32>,
    n_gpu_layers: Option<i32>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    let params = if role == "worker" {
        &mut config.harness_role_params.worker
    } else if role == "orchestrator" {
        &mut config.harness_role_params.orchestrator
    } else {
        return Err(format!("Unknown role: {role}"));
    };
    params.ctx_size = ctx_size.filter(|&c| c > 0);
    params.n_gpu_layers = n_gpu_layers;
    config.save().map_err(|e| e.to_string())
}

/// Attach/detach a companion file for one router role.
#[tauri::command]
#[specta::specta]
pub async fn set_role_attachment(
    role: String,
    kind: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    let params = if role == "worker" {
        &mut config.harness_role_params.worker
    } else if role == "orchestrator" {
        &mut config.harness_role_params.orchestrator
    } else {
        return Err(format!("Unknown role: {role}"));
    };
    match kind.as_str() {
        "mmproj" => params.no_mmproj = !enabled,
        "chat-template" => params.no_chat_template = !enabled,
        "draft model" => params.no_draft = !enabled,
        other => return Err(format!("Unknown attachment: {other}")),
    }
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_max_turns(
    orchestrator: u32,
    subagent: u32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.harness_max_turns = orchestrator.clamp(1, 500);
    config.harness_subagent_max_turns = subagent.clamp(1, 200);
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_verify_mode(
    mode: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mode = match mode.to_ascii_lowercase().as_str() {
        "off" => harness::agent::VerifyMode::Off,
        "normal" => harness::agent::VerifyMode::Normal,
        "thorough" => harness::agent::VerifyMode::Thorough,
        other => return Err(format!("Invalid verify mode: {other}")),
    };
    let mut config = state.config.lock().unwrap();
    config.verify_mode = mode;
    config.save().map_err(|e| e.to_string())
}

/// Override the system prompt (empty resets to built-in); next run.
#[tauri::command]
#[specta::specta]
pub async fn set_system_prompt(
    prompt: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    let trimmed = prompt.trim().to_string();
    config.harness_system_prompt = if trimmed.is_empty() { None } else { Some(trimmed) };
    config.save().map_err(|e| e.to_string())
}

/// Replace the named system prompt presets (max 5, names trimmed).
#[tauri::command]
#[specta::specta]
pub async fn set_system_prompt_presets(
    presets: Vec<crate::config::SystemPromptPreset>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let cleaned: Vec<_> = presets
        .into_iter()
        .take(5)
        .filter(|p| !p.name.trim().is_empty())
        .map(|p| crate::config::SystemPromptPreset {
            name: p.name.trim().chars().take(40).collect(),
            prompt: p.prompt,
        })
        .collect();
    let mut config = state.config.lock().unwrap();
    config.system_prompt_presets = cleaned;
    config.save().map_err(|e| e.to_string())
}

/// Toggle language-server diagnostics and the `lsp` tool for the next run.
#[tauri::command]
#[specta::specta]
pub async fn set_lsp_enabled(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    {
        let mut config = state.config.lock().unwrap();
        config.lsp_enabled = enabled;
        config.save().map_err(|e| e.to_string())?;
    }
    if !enabled {
        state.lsp.shutdown_all();
    }
    Ok(())
}

/// Enable/disable one agent tool for future runs.
#[tauri::command]
#[specta::specta]
pub async fn set_agent_tool_enabled(
    name: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !harness::tools::ToolRegistry::builtin_tools_info()
        .iter()
        .any(|t| t.name == name)
    {
        return Err(format!("Unknown agent tool: {name}"));
    }
    let mut config = state.config.lock().unwrap();
    config.agent_tools_disabled.retain(|n| n != &name);
    if !enabled {
        config.agent_tools_disabled.push(name);
        config.agent_tools_disabled.sort();
    }
    config.save().map_err(|e| e.to_string())
}

/// Replace the configured stdio language servers.
#[tauri::command]
#[specta::specta]
pub async fn set_lsp_servers(
    servers: Vec<crate::config::LspServerConfig>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let specs = {
        let mut config = state.config.lock().unwrap();
        config.lsp_servers = Some(servers);
        config.save().map_err(|e| e.to_string())?;
        crate::lsp_specs(&config)
    };
    state.lsp.set_specs(specs);
    Ok(())
}

/// Restore the built-in language-server list.
#[tauri::command]
#[specta::specta]
pub async fn reset_lsp_servers(
    state: State<'_, AppState>,
) -> Result<Vec<crate::config::LspServerConfig>, String> {
    let servers = crate::config::default_lsp_servers();
    let specs = {
        let mut config = state.config.lock().unwrap();
        config.lsp_servers = Some(servers.clone());
        config.save().map_err(|e| e.to_string())?;
        crate::lsp_specs(&config)
    };
    state.lsp.set_specs(specs);
    Ok(servers)
}

/// One command per sound to avoid clobbering via stale snapshots.
#[tauri::command]
#[specta::specta]
pub async fn set_sound_agent(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.sound_agent = enabled;
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_sound_permissions(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.sound_permissions = enabled;
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_sound_errors(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.sound_errors = enabled;
    config.save().map_err(|e| e.to_string())
}

// ── Tools + skills listing ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct BuiltinToolDto {
    pub name: String,
    pub summary: String,
    pub approval: String,
    pub note: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PluginDto {
    pub name: String,
    pub description: String,
    pub scope: String,
    pub dir: String,
    pub enabled: bool,
    pub approval: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PluginErrorDto {
    pub dir: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ToolsList {
    pub builtin: Vec<BuiltinToolDto>,
    pub plugins: Vec<PluginDto>,
    pub plugin_errors: Vec<PluginErrorDto>,
    pub plugins_dir: String,
}

#[tauri::command]
#[specta::specta]
pub async fn tools_list(state: State<'_, AppState>) -> Result<ToolsList, String> {
    let (lsp_enabled, agent_disabled) = {
        let config = state.config.lock().unwrap();
        (config.lsp_enabled, config.agent_tools_disabled.clone())
    };
    let builtin = harness::tools::ToolRegistry::builtin_tools_info()
        .into_iter()
        .filter(|t| lsp_enabled || t.name != "lsp")
        .map(|t| BuiltinToolDto {
            name: t.name.to_string(),
            summary: t.summary.to_string(),
            approval: t.approval.to_string(),
            note: t.note.to_string(),
            enabled: !agent_disabled.iter().any(|d| d == t.name),
        })
        .collect();
    let disabled = state.config.lock().unwrap().plugin_disabled.clone();
    let mut roots: Vec<(String, PathBuf)> = Vec::new();
    if let Some(data) = crate::config::data_dir() {
        roots.push(("global".to_string(), data.join("werk").join("plugins")));
    }
    if let Ok(root) = project_root(&state) {
        roots.push(("project".to_string(), root.join(".werk").join("plugins")));
    }
    let (plugins, plugin_errors) = harness::plugins::discover(&roots);
    let plugins = plugins
        .into_iter()
        .map(|p| PluginDto {
            enabled: !disabled.iter().any(|d| d == &p.name),
            dir: p.dir.to_string_lossy().to_string(),
            approval: p.approval,
            name: p.name,
            description: p.description,
            scope: p.scope,
        })
        .collect();
    let plugin_errors = plugin_errors
        .into_iter()
        .map(|e| PluginErrorDto { dir: e.dir, error: e.error })
        .collect();
    let plugins_dir = crate::config::data_dir()
        .map(|d| d.join("werk").join("plugins").to_string_lossy().to_string())
        .unwrap_or_default();
    if !plugins_dir.is_empty() {
        let _ = std::fs::create_dir_all(&plugins_dir);
    }
    Ok(ToolsList { builtin, plugins, plugin_errors, plugins_dir })
}

#[tauri::command]
#[specta::specta]
pub async fn plugin_set_enabled(
    name: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Plugin name must not be empty".to_string());
    }
    let mut config = state.config.lock().unwrap();
    if enabled {
        config.plugin_disabled.retain(|d| d != &name);
    } else if !config.plugin_disabled.iter().any(|d| d == &name) {
        config.plugin_disabled.push(name);
    }
    config.save().map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    pub scope: String,
    pub dir: String,
}

#[tauri::command]
#[specta::specta]
pub async fn skills_list(state: State<'_, AppState>) -> Result<Vec<SkillDto>, String> {
    let mut roots = Vec::new();
    if let Some(data) = crate::config::data_dir() {
        roots.push(("global".to_string(), data.join("werk").join("skills")));
    }
    if let Ok(root) = project_root(&state) {
        roots.push(("project".to_string(), root.join(".werk").join("skills")));
    }
    let mut out = Vec::new();
    for (scope, root) in roots {
        for s in harness::skills::discover(&[root]) {
            out.retain(|e: &SkillDto| e.name != s.name);
            out.push(SkillDto {
                name: s.name,
                description: s.description,
                scope: scope.clone(),
                dir: s.path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// One editable memory file: where it lives and what it says.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct MemoryFileDto {
    pub scope: String,
    pub path: String,
    pub exists: bool,
    pub text: String,
}

fn memory_file(scope: &str, state: &AppState) -> Result<MemoryFileDto, String> {
    let path = match scope {
        "global" => global_base_dir()
            .map(|b| b.join("MEMORY.md"))
            .ok_or_else(|| "Cannot find data directory".to_string())?,
        "project" => {
            let root = project_root(state).map_err(|e| e.to_string())?;
            harness::memory::project_memory_path(&root)
        }
        _ => return Err("Unknown memory scope".to_string()),
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    Ok(MemoryFileDto {
        scope: scope.into(),
        path: path.to_string_lossy().to_string(),
        exists: path.exists(),
        text,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn harness_memory_get(
    scope: String,
    state: State<'_, AppState>,
) -> Result<MemoryFileDto, String> {
    memory_file(&scope, &state)
}

#[tauri::command]
#[specta::specta]
pub async fn harness_memory_set(
    scope: String,
    text: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let path = memory_file(&scope, &state)?.path;
    if text.len() > harness::memory::MEMORY_FILE_CAP {
        return Err(format!(
            "Memory file too large ({} KiB max) — trim it first",
            harness::memory::MEMORY_FILE_CAP / 1024
        ));
    }
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Platform idiom for corner/shape theming (no inputs needed).
#[tauri::command]
#[specta::specta]
pub async fn get_platform_style() -> Result<PlatformStyle, String> {
    Ok(PlatformStyle::detect())
}

/// OS + desktop idiom for corner/shape theming.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PlatformStyle {
    pub os: String,
    /// "sharp" (Windows 10, KDE) or "round" (macOS, Windows 11, GNOME).
    pub corners: String,
    pub desktop: String,
}

impl PlatformStyle {
    pub fn detect() -> Self {
        let os = std::env::consts::OS.to_string();
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
        #[cfg(target_os = "macos")]
        let corners = "round".to_string();
        #[cfg(target_os = "windows")]
        let corners = if windows_build() >= 22000 { "round".to_string() } else { "sharp".to_string() };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let corners = if desktop.contains("gnome") {
            "round".to_string()
        } else {
            "sharp".to_string()
        };
        Self { os, corners, desktop }
    }
}

#[cfg(target_os = "windows")]
fn windows_build() -> u32 {
    sysinfo::System::os_version()
        .and_then(|v| v.split('.').nth(2)?.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_only_suggests_delegation_in_router_mode() {
        let single =
            system_prompt_for(harness::agent::VerifyMode::Normal, crate::config::ServerMode::Single);
        let external =
            system_prompt_for(harness::agent::VerifyMode::Normal, crate::config::ServerMode::External);
        let router =
            system_prompt_for(harness::agent::VerifyMode::Normal, crate::config::ServerMode::Router);
        assert!(!single.contains("subagent"), "{single}");
        assert!(!external.contains("subagent"), "{external}");
        assert!(router.contains("researcher subagent"), "{router}");
        // Verification off drops the paragraph entirely.
        let off = system_prompt_for(harness::agent::VerifyMode::Off, crate::config::ServerMode::Router);
        assert!(!off.contains("Verify load-bearing"), "{off}");
    }

    #[test]
    fn base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
    }

    #[test]
    fn attachments_fold_into_message() {
        let (msg, images) = with_attachments(
            "hi",
            &[
                SendAttachment {
                    name: "a.png".into(),
                    kind: "image".into(),
                    path: None,
                    data_base64: Some("AA==".into()),
                    text: None,
                },
                SendAttachment {
                    name: "n.txt".into(),
                    kind: "text".into(),
                    path: None,
                    data_base64: None,
                    text: Some("body".into()),
                },
            ],
        );
        assert_eq!(msg.role, "user");
        assert_eq!(images.len(), 1);
        assert!(images[0].starts_with("data:image/png;base64,"));
        let content = msg.content.unwrap();
        assert!(content.as_array().unwrap().len() == 2);
        assert!(content.to_string().contains("n.txt"));
    }

    #[test]
    fn history_splits_parts() {
        let msg = ChatMessage {
            role: "user".into(),
            content: Some(serde_json::json!([
                {"type": "text", "text": "hi"},
                {"type": "image_url", "image_url": {"url": "data:x"}},
            ])),
            tool_calls: None,
            tool_call_id: None,
        };
        let view = to_history_message(&msg);
        assert_eq!(view.content.as_deref(), Some("hi"));
        assert_eq!(view.images, Some(vec!["data:x".to_string()]));
        let plain = ChatMessage::user("yo");
        let view = to_history_message(&plain);
        assert_eq!(view.content.as_deref(), Some("yo"));
        assert_eq!(view.images, None);
    }

    #[test]
    fn git_maps_classify_changes() {
        let dir = std::env::temp_dir().join(format!("werk-changes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            crate::hidden::command("git")
                .args(["-C"])
                .arg(&dir)
                .args(args)
                .output()
                .expect("git runs")
        };
        let init = git(&["init", "-q"]);
        if !init.status.success() {
            // git unavailable: nothing to verify.
            return;
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.join("c.txt"), "gone\n").unwrap();
        assert!(git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "add",
            "-A"
        ])
        .status
        .success());
        assert!(git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init"
        ])
        .status
        .success());
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("b.txt"), "x\ny\nz\n").unwrap();
        std::fs::remove_file(dir.join("c.txt")).unwrap();

        let statuses = git_status_map(&dir).expect("status map");
        assert_eq!(statuses.get("a.txt").map(String::as_str), Some("M"));
        assert_eq!(statuses.get("b.txt").map(String::as_str), Some("??"));
        assert_eq!(statuses.get("c.txt").map(String::as_str), Some("D"));

        let numstat = git_numstat_map(&dir, None);
        assert_eq!(numstat.get("a.txt"), Some(&(1, 0)));
        assert_eq!(numstat.get("c.txt"), Some(&(0, 1)));
        assert_eq!(count_file_lines(&dir.join("b.txt")), 3);

        // Committing mid-run must not hide the change: diffing against the
        // HEAD captured before the run still reports it.
        let base = git_head(&dir).expect("head");
        assert!(git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"])
            .status
            .success());
        assert!(git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "changes"
        ])
        .status
        .success());
        assert!(git_status_map(&dir).unwrap().is_empty(), "clean tree after commit");
        let names = git_name_status(&dir, Some(&base));
        assert_eq!(names.get("a.txt").map(String::as_str), Some("M"));
        assert_eq!(names.get("b.txt").map(String::as_str), Some("A"));
        assert_eq!(names.get("c.txt").map(String::as_str), Some("D"));
        let numstat = git_numstat_map(&dir, Some(&base));
        assert_eq!(numstat.get("a.txt"), Some(&(1, 0)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
