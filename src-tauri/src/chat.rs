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
    /// Most recent session delete, held in memory so the UI can undo it.
    pub deleted_session: Mutex<Option<DeletedSession>>,
    /// Orchestrator task list for this session (prompt state, not history).
    pub todos: harness::todos::TodoList,
    /// Last harness activity, for the idle server unload.
    pub last_activity: Mutex<std::time::Instant>,
    /// Stop flag for the in-flight run; per runtime so Chat and Talk are
    /// independent.
    pub abort: Arc<std::sync::atomic::AtomicBool>,
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
            deleted_session: Mutex::new(None),
            todos: std::sync::Arc::new(Mutex::new(Vec::new())),
            last_activity: Mutex::new(std::time::Instant::now()),
            abort: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

/// Undo window for a deleted session; the file itself already sits in the
/// OS recycle bin after that, so this only gates the in-app button.
const SESSION_UNDO_WINDOW: std::time::Duration = std::time::Duration::from_secs(600);

pub struct DeletedSession {
    file: SessionFile,
    at: std::time::Instant,
}

fn undo_window_open(at: std::time::Instant) -> bool {
    at.elapsed() < SESSION_UNDO_WINDOW
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

/// Builtin tools the built-in prompt may reference. Disabled ones are left
/// out entirely, so the model is never told about a tool it cannot call.
#[derive(Debug, Clone, Copy)]
struct PromptTools {
    read_file: bool,
    write_file: bool,
    edit_file: bool,
    find_files: bool,
    search_content: bool,
    exec: bool,
    remember: bool,
    ask_user: bool,
    spawn_subagent: bool,
    get_time: bool,
    todo: bool,
}

impl PromptTools {
    /// Everything enabled; the Settings preview of the default prompt.
    fn all() -> Self {
        Self {
            read_file: true,
            write_file: true,
            edit_file: true,
            find_files: true,
            search_content: true,
            exec: true,
            remember: true,
            ask_user: true,
            spawn_subagent: true,
            get_time: true,
            todo: true,
        }
    }

    fn from_disabled(disabled: &[String]) -> Self {
        let on = |name: &str| !disabled.iter().any(|d| d == name);
        Self {
            read_file: on("read_file"),
            write_file: on("write_file"),
            edit_file: on("edit_file"),
            find_files: on("find_files"),
            search_content: on("search_content"),
            exec: on("exec"),
            remember: on("remember"),
            ask_user: on("ask_user"),
            spawn_subagent: on("spawn_subagent"),
            get_time: on("get_time"),
            todo: on("todo"),
        }
    }
}

/// Router mode has a worker model to delegate to; single and external modes
/// don't, so their prompt never suggests subagents. A disabled
/// `spawn_subagent` (or `remember`) drops the mention too.
fn verify_protocol(delegate: bool, remember: bool) -> String {
    let fallback = if delegate {
        "or one researcher subagent"
    } else {
        "or one direct read/search"
    };
    let save = if remember { " (save facts with remember)" } else { "" };
    format!(
        "Verify load-bearing claims you recall from training (API defaults, flags, version behavior) \
        before acting: one direct read/search, {fallback}. Never verify trivia or the same fact \
        twice{save}; one check, then proceed."
    )
}

fn system_prompt_for(
    mode: harness::agent::VerifyMode,
    server_mode: crate::config::ServerMode,
    tools: PromptTools,
) -> String {
    let delegate = server_mode == crate::config::ServerMode::Router && tools.spawn_subagent;
    let verify = if mode == harness::agent::VerifyMode::Off {
        String::new()
    } else {
        verify_protocol(delegate, tools.remember)
    };

    let mut parts: Vec<String> = vec![
        "You are Werk's agent in a sandboxed project directory; relative paths resolve there."
            .to_string(),
    ];

    let mut file_tools: Vec<&str> = Vec::new();
    if tools.read_file {
        file_tools.push("read_file");
    }
    if tools.write_file {
        file_tools.push("write_file");
    }
    if tools.edit_file {
        file_tools.push("edit_file");
    }
    if tools.find_files {
        file_tools.push("find_files");
    }
    if tools.search_content {
        file_tools.push("search_content");
    }
    if !file_tools.is_empty() {
        let exec_clause = if tools.exec {
            " — exec is only for programs (builds, tests, git, servers)."
        } else {
            "."
        };
        parts.push(format!(
            "Use the file tools for all file work ({}). Never use shell commands for files{exec_clause}",
            file_tools.join(", ")
        ));
    }
    parts.push(
        "Images only reach you when attached to the conversation; you cannot view image files from \
         disk."
            .to_string(),
    );
    if tools.exec {
        parts.push(harness::agent::os_shell_snippet());
    }
    parts.push(
        "Reads run free; writes and shell commands may need approval — if denied, adapt, don't retry."
            .to_string(),
    );
    if tools.remember {
        parts.push(
            "Save durable preferences and corrections with remember (project scope; global only for \
             the user themselves). Keep memories short."
                .to_string(),
        );
    }
    if tools.ask_user {
        parts.push(
            "Use ask_user (2-4 options) when genuinely blocked on a decision; never for facts you can \
             look up."
                .to_string(),
        );
    }
    if tools.todo {
        parts.push(
            "For multi-step work, keep a short task list with the todo tool and update it as you \
             go."
                .to_string(),
        );
    }
    if tools.get_time {
        parts.push("Call get_time before date-sensitive work (search, releases, 'latest').".to_string());
    }
    if !verify.trim().is_empty() {
        parts.push(verify.trim().to_string());
    }
    parts.push("Read before editing; make small exact edits; summarize concisely when done.".to_string());
    parts.join(" ")
}

// ── Approval gate ─────────────────────────────────────────────────────────

/// Agent-mode system prompt: custom override or the built-in, plus the
/// subagent block when delegation is available.
fn agent_prompt_base(
    state: &AppState,
    app_config: &crate::config::AppConfig,
    mode: crate::config::ServerMode,
    subagent_targets: &[String],
    prompt_tools: PromptTools,
) -> String {
    let c = state.config.lock().unwrap();
    let verify = c.verify_mode;
    let server_mode = c.server_mode;
    let mut base = c
        .harness_system_prompt
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(move || system_prompt_for(verify, server_mode, prompt_tools));
    if prompt_tools.spawn_subagent
        && (mode == crate::config::ServerMode::Router || !subagent_targets.is_empty())
    {
        // Router: the worker (or orchestrator) is the default; external
        // mode's default is the run model. Single mode stays silent.
        let (label, vision) = if mode == crate::config::ServerMode::Router {
            let sub_model = app_config
                .harness_roles
                .worker
                .clone()
                .or_else(|| app_config.harness_roles.orchestrator.clone());
            let label = sub_model
                .as_deref()
                .map(crate::server::file_stem_or_self)
                .unwrap_or_else(|| "the main model".to_string());
            // Mixed mode: an external default's vision is unknown.
            let external = sub_model
                .as_deref()
                .map(|p| {
                    crate::config::Provider::split_target(p, &app_config.providers).is_some()
                })
                .unwrap_or(false);
            let vision = if external {
                None
            } else {
                sub_model.as_deref().map(|p| {
                    crate::server::find_mmproj_sibling(std::path::Path::new(p)).is_some()
                })
            };
            (Some(label), vision)
        } else {
            (None, None)
        };
        base.push_str(&subagent_prompt_block(
            subagent_targets,
            label.as_deref(),
            vision,
        ));
    }
    base
}

/// Plain text of the last few messages, for keyword-matching lorebook entries.
fn recent_transcript(history: &[ChatMessage]) -> String {
    let mut out = String::new();
    for msg in history.iter().rev().take(10) {
        let text = match &msg.content {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };
        out = format!("{text}\n{out}");
        if out.len() > 8_000 {
            break;
        }
    }
    out
}


struct UiGate {
    app: AppHandle,
    runtime: Arc<HarnessRuntime>,
    /// Event carrying ask_user questions ("harness_question"/"talk_question").
    question_event: &'static str,
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
        let question_event = self.question_event;
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
                question_event,
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
    todos: &harness::todos::TodoList,
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
    registry = registry.add(Arc::new(harness::todos::TodoTool::new(todos.clone())));
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

/// Subagent paragraph for router/external prompts: model choices, the
/// default (so the list and the default can't read as a contradiction), the
/// specialist kinds, and the image argument.
fn subagent_prompt_block(
    targets: &[String],
    default_label: Option<&str>,
    vision: Option<bool>,
) -> String {
    let mut text =
        String::from("\n\nSubagents share the sandbox and never see this conversation. ");
    if !targets.is_empty() {
        text.push_str(&format!(
            "Models: {} — pass one as spawn_subagent's `model`; ",
            targets.join(", ")
        ));
    }
    match default_label {
        Some(label) => text.push_str(&format!(
            "the default is {label} (vision: {}). ",
            match vision {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            }
        )),
        None => text.push_str("the default is the main model. "),
    }
    text.push_str(
        "'coder' reads/writes/edits and runs commands; 'researcher' is read-only. For an image, \
         pass `image: <path>` (needs vision) — subagents cannot open image files themselves.",
    );
    text
}

/// Router preset entries for the configured roles. Role ctx overrides win
/// over the launch `base_ctx`; sibling templates auto-attach unless the
/// launch pins an explicit template.
pub fn router_role_entries(
    roles: &crate::config::HarnessRoles,
    params: &crate::config::HarnessRoleParams,
    providers: &[crate::config::Provider],
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
        // Mixed mode: external roles never enter the preset.
        if crate::config::Provider::split_target(&path, providers).is_some() {
            continue;
        }
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
    Ok(system_prompt_for(verify, server_mode, PromptTools::all()))
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
    /// Orchestrator task list at last save; prompt state, not transcript.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    todos: Vec<harness::todos::TodoItem>,
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
    #[serde(default)]
    pub todos: Vec<TodoDto>,
}

/// One checkpoint in the orchestrator's task list.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct TodoDto {
    pub text: String,
    pub done: bool,
}

fn todo_dtos(items: &[harness::todos::TodoItem]) -> Vec<TodoDto> {
    items
        .iter()
        .map(|t| TodoDto { text: t.text.clone(), done: t.done })
        .collect()
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

/// Transcript path for a runtime: agent sessions live in the sessions dir,
/// roleplay threads one per character under the roleplay data dir.
fn transcript_path(id: &str, roleplay: bool) -> Option<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    if roleplay {
        crate::roleplay::roleplay_dir().map(|d| d.join("sessions").join(format!("{id}.json")))
    } else {
        sessions_dir().map(|d| d.join(format!("{id}.json")))
    }
}

/// Stable talk session id: the active card, or `general`.
fn talk_session_id(app_config: &crate::config::AppConfig) -> String {
    app_config
        .roleplay
        .card_id
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "general".to_string())
}

fn save_transcript_current(
    state: &AppState,
    runtime: &HarnessRuntime,
    roleplay: bool,
    title: Option<String>,
) -> Result<()> {
    let messages = runtime.history.lock().unwrap().clone();
    save_transcript(state, runtime, &messages, title, roleplay)
}

/// Snapshot a transcript for the open session. Also called right after the
/// first send so a new chat shows up in the sessions list while it streams.
fn save_transcript(
    state: &AppState,
    runtime: &HarnessRuntime,
    messages: &[ChatMessage],
    title: Option<String>,
    roleplay: bool,
) -> Result<()> {
    let id = {
        let sid = runtime.session_id.lock().unwrap().clone();
        match sid {
            Some(id) => id,
            None => return Ok(()),
        }
    };
    let Some(path) = transcript_path(&id, roleplay) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let meta = runtime.meta.lock().unwrap().clone();
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
    let project = if roleplay {
        None
    } else {
        state.config.lock().unwrap().harness_active_project.clone()
    };
    let todos = if roleplay {
        Vec::new()
    } else {
        runtime.todos.lock().unwrap().clone()
    };
    let file = SessionFile {
        id,
        title,
        project,
        created,
        updated: now,
        messages: messages.to_vec(),
        meta,
        todos,
    };
    std::fs::write(&path, serde_json::to_string_pretty(&file)?)?;
    Ok(())
}

fn truncate_meta(runtime: &HarnessRuntime, len: usize) {
    runtime.meta.lock().unwrap().retain(|&i, _| i < len);
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
fn shift_meta_for_compaction(runtime: &HarnessRuntime, cuts: &[usize]) {
    if cuts.is_empty() {
        return;
    }
    let mut meta = runtime.meta.lock().unwrap();
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

/// Send a message to the coding agent; streams over `harness_event`.
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_send(
    message: String,
    reasoning_effort: Option<String>,
    attachments: Option<Vec<SendAttachment>>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RunResult, String> {
    let runtime = state.harness.clone();
    agent_send_impl(
        state,
        app,
        runtime,
        false,
        "harness_event",
        message,
        reasoning_effort,
        attachments,
    )
    .await
}

/// Send a message in the roleplay profile; streams over `talk_event`.
#[tauri::command]
#[specta::specta]
pub async fn talk_send(
    message: String,
    reasoning_effort: Option<String>,
    attachments: Option<Vec<SendAttachment>>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RunResult, String> {
    let runtime = state.talk.clone();
    agent_send_impl(
        state,
        app,
        runtime,
        true,
        "talk_event",
        message,
        reasoning_effort,
        attachments,
    )
    .await
}

async fn agent_send_impl(
    state: State<'_, AppState>,
    app: AppHandle,
    runtime: Arc<HarnessRuntime>,
    roleplay: bool,
    event_name: &'static str,
    message: String,
    reasoning_effort: Option<String>,
    attachments: Option<Vec<SendAttachment>>,
) -> Result<RunResult, String> {
    if runtime
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("An agent run is already in progress".to_string());
    }
    let _running_guard = RunningGuard(runtime.clone());
    // A stale Stop from the previous run must not kill this one.
    runtime.abort.store(false, Ordering::SeqCst);
    *runtime.last_activity.lock().unwrap() = std::time::Instant::now();
    runtime.steering.lock().unwrap().clear();

    if !roleplay {
        load_permission_grants(&state);
    }
    let root = project_root(&state).map_err(|e| e.to_string())?;
    let global_base = global_base_dir();
    let app_config = state.config.lock().unwrap().clone();
    // The jail hides the agent's own instruction files and shields sensitive
    // paths (built-ins + .gitignore + the user list).
    let jail = PathJail::new(&root, &[])
        .map_err(|e| e.to_string())?
        .hide_agent_files(app_config.agent_files_hidden);
    let sensitive = Arc::new(harness::sensitive::SensitivePolicy::build(
        jail.root(),
        &app_config.sensitive_patterns,
        &app_config.sensitive_allow,
    ));
    let jail = Arc::new(jail.shield(sensitive));
    let disabled = app_config.plugin_disabled.clone();
    let lsp = app_config.lsp_enabled.then(|| state.lsp.clone());
    let (mcp_tools, mcp_errors) = if roleplay {
        (Vec::new(), Vec::new())
    } else {
        // MCP tools run in the harness (all modes): spawn servers once, reuse
        // the manager across runs; servers are gated by per-tool approval.
        discover_mcp_tools(&state, &app_config).await
    };
    for text in mcp_errors {
        let _ = app.emit(
            event_name,
            serde_json::json!({"type": "notice", "text": text}),
        );
    }
    let registry = if roleplay {
        let memory = crate::roleplay::character_memory_path(app_config.roleplay.card_id.as_deref())
            .ok_or_else(|| "Cannot find data directory".to_string())?;
        ToolRegistry::project_tools(jail.clone())
            .only(&[])
            .add(Arc::new(harness::memory::RememberTool::for_file(memory)))
            .add(Arc::new(harness::tools::AskUserTool))
    } else {
        build_registry(
            jail.clone(),
            &root,
            global_base.as_deref(),
            &disabled,
            &app_config.agent_tools_disabled,
            &runtime.todos,
            lsp,
            mcp_tools,
        )
    };

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
            &app_config.providers,
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
    if runtime.history.lock().unwrap().is_empty() {
        if let Some(id) = runtime.session_id.lock().unwrap().clone() {
            if let Some(path) = transcript_path(&id, roleplay) {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(saved) = serde_json::from_str::<SessionFile>(&text) {
                        *runtime.history.lock().unwrap() = saved.messages;
                        *runtime.meta.lock().unwrap() = saved.meta;
                    }
                }
            }
        }
    }
    let mut history = std::mem::take(&mut *runtime.history.lock().unwrap());
    // Subagent model choices the prompt may advertise.
    let subagent_targets: Vec<String> = match mode {
        crate::config::ServerMode::External => app_config
            .provider_favorites
            .iter()
            .filter(|t| crate::config::Provider::split_target(t, &app_config.providers).is_some())
            .cloned()
            .collect(),
        // Model ids, not role names: the choices are what `model` takes, and
        // role names read like `agent_type` values. Mixed roles advertise
        // their `provider:model` target instead.
        crate::config::ServerMode::Router => {
            let mut ids: Vec<String> = ["orchestrator", "worker"]
                .iter()
                .filter_map(|role| {
                    let path = if *role == "orchestrator" {
                        app_config.harness_roles.orchestrator.clone()
                    } else {
                        app_config.harness_roles.worker.clone()
                    };
                    path.filter(|p| !p.trim().is_empty()).map(|p| {
                        if crate::config::Provider::split_target(&p, &app_config.providers)
                            .is_some()
                        {
                            p
                        } else {
                            model_id(&p)
                        }
                    })
                })
                .collect();
            ids.dedup();
            ids
        }
        crate::config::ServerMode::Single => Vec::new(),
    };
    if !history.iter().any(|m| m.role == "system") {
        if roleplay {
            let card = app_config
                .roleplay
                .card_id
                .as_deref()
                .and_then(|id| crate::roleplay::load_card(id).ok());
            let mut base = crate::roleplay::system_prompt(&app_config, card.as_ref());
            if let Some(card) = &card {
                let user_name = crate::roleplay::display_user_name(&app_config.roleplay);
                let world =
                    crate::roleplay::lorebook_block(card, &recent_transcript(&history), &user_name);
                if !world.is_empty() {
                    base.push_str("\n\n");
                    base.push_str(&world);
                }
            }
            let memory = crate::roleplay::memory_block(&app_config);
            history.insert(0, ChatMessage::system(format!("{base}{memory}")));
        } else {
            let prompt_tools = PromptTools::from_disabled(&app_config.agent_tools_disabled);
            let base = agent_prompt_base(&state, &app_config, mode, &subagent_targets, prompt_tools);
            let memory = harness::memory::load_block(
                global_base.as_deref().map(|b| b.join("MEMORY.md")).as_deref(),
                &root,
            );
            let run_context = crate::run_context::run_context_block(&root);
            history.insert(
                0,
                ChatMessage::system(format!(
                    "{base}\n\nProject directory: {}{run_context}{memory}",
                    root.display()
                )),
            );
        }
    }
    let (user_msg, _attached_images) = with_attachments(&message, &attachments.unwrap_or_default());
    history.push(user_msg);
    if runtime.session_id.lock().unwrap().is_none() {
        let id = if roleplay {
            talk_session_id(&app_config)
        } else {
            let base = format!("s{}", now_secs());
            let mut id = base.clone();
            let mut n = 2;
            while session_path(&id).map(|p| p.exists()).unwrap_or(false) {
                id = format!("{base}-{n}");
                n += 1;
            }
            id
        };
        *runtime.session_id.lock().unwrap() = Some(id);
    }
    // Persist the sent transcript right away so the new chat appears in the
    // sessions list while the first reply streams (and survives a crash).
    if let Err(e) = save_transcript(&state, &runtime, &history, None, roleplay) {
        let _ = app.emit(
            event_name,
            serde_json::json!({"type": "notice", "text": format!("Session save failed: {e}")}),
        );
    }

    // External API mode resolves the configured provider target; local modes
    // never mix in external endpoints.
    let orch_target = if mode == crate::config::ServerMode::External {
        let target = orchestrator_id.as_deref().ok_or_else(|| {
            "External API mode: no model selected - pick a provider and model on the Mode page"
                .to_string()
        })?;
        let (p, m) = crate::config::Provider::split_target(target, &app_config.providers)
            .ok_or_else(|| {
                format!(
                    "External API mode: \"{target}\" has no configured provider (check the Mode tab)"
                )
            })?;
        Some((p.clone(), m.to_string()))
    } else if router {
        // Mixed mode: an external orchestrator chats through its provider.
        orchestrator_id
            .as_deref()
            .and_then(|t| crate::config::Provider::split_target(t, &app_config.providers))
            .map(|(p, m)| (p.clone(), m.to_string()))
    } else {
        None
    };

    // One HTTP client per external provider in use (orchestrator, utility,
    // subagent favorites), built before borrowing.
    let mut external: Vec<(String, LlmClient)> = Vec::new();
    for (provider, _) in orch_target.iter() {
        ensure_external_client(&mut external, provider);
    }
    if let Some((p, _)) = app_config
        .utility_target
        .as_deref()
        .and_then(|t| crate::config::Provider::split_target(t, &app_config.providers))
    {
        ensure_external_client(&mut external, p);
    }
    for fav in &app_config.provider_favorites {
        if let Some((p, _)) = crate::config::Provider::split_target(fav, &app_config.providers) {
            ensure_external_client(&mut external, p);
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
    // A router worker is required once configured; in external mode the local
    // worker is optional, so a stopped server just drops the choice.
    if router && worker_path.is_some() && running_port.is_none() {
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

    // Mixed mode: an external worker becomes the default subagent model.
    let worker_external = worker_path
        .as_deref()
        .and_then(|p| crate::config::Provider::split_target(p, &app_config.providers))
        .map(|(p, m)| (p.clone(), m.to_string()));
    let (worker_client, worker_model, worker_limit) = match &worker_external {
        Some((provider, model)) => (
            Some(external_client(provider)),
            Some(model.clone()),
            provider.context_length.map(u64::from),
        ),
        None => (
            None,
            worker_path.clone().map(|p| model_id(&p)),
            worker_path.as_deref().and_then(|w| {
                role_context_limit(
                    app_config.harness_role_params.worker.ctx_size,
                    launch_n_ctx,
                    Some(w),
                )
            }),
        ),
    };

    // Subagent model overrides: external favorites, or the router roles —
    // either of which may itself be an external favorite (mixed mode).
    let mut subagent_choices: Vec<harness::agent::SubagentChoice> = Vec::new();
    match mode {
        crate::config::ServerMode::External => {
            for target in &subagent_targets {
                if let Some((p, m)) =
                    crate::config::Provider::split_target(target, &app_config.providers)
                {
                    subagent_choices.push(harness::agent::SubagentChoice {
                        target: target.clone(),
                        model: Some(m.to_string()),
                        client: Some(external_client(p)),
                        // Remote vision is unknown; let the provider decide.
                        vision: None,
                        context_limit: None,
                    });
                }
            }
        }
        crate::config::ServerMode::Router => {
            for role in ["orchestrator", "worker"] {
                let path = if role == "orchestrator" {
                    app_config.harness_roles.orchestrator.clone()
                } else {
                    app_config.harness_roles.worker.clone()
                };
                let Some(path) = path.filter(|p| !p.trim().is_empty()) else { continue };
                let params = if role == "orchestrator" {
                    &app_config.harness_role_params.orchestrator
                } else {
                    &app_config.harness_role_params.worker
                };
                // Mixed mode: an external role runs on its provider.
                if let Some((provider, model)) =
                    crate::config::Provider::split_target(&path, &app_config.providers)
                {
                    let target = path.clone();
                    if subagent_choices
                        .iter()
                        .any(|c| c.target.eq_ignore_ascii_case(&target))
                    {
                        continue;
                    }
                    subagent_choices.push(harness::agent::SubagentChoice {
                        target,
                        model: Some(model.to_string()),
                        client: Some(external_client(provider)),
                        vision: None,
                        context_limit: provider.context_length.map(u64::from),
                    });
                    continue;
                }
                let id = model_id(&path);
                if subagent_choices.iter().any(|c| c.target.eq_ignore_ascii_case(&id)) {
                    continue;
                }
                subagent_choices.push(harness::agent::SubagentChoice {
                    target: id.clone(),
                    model: Some(id),
                    client: None,
                    vision: Some(
                        crate::server::find_mmproj_sibling(std::path::Path::new(&path)).is_some(),
                    ),
                    context_limit: role_context_limit(
                        params.ctx_size,
                        launch_n_ctx,
                        Some(&path),
                    ),
                });
            }
        }
        _ => {}
    }
    let subagent_vision = if mode == crate::config::ServerMode::External {
        None
    } else {
        let path = app_config
            .harness_roles
            .worker
            .clone()
            .or_else(|| app_config.harness_roles.orchestrator.clone())
            .or_else(|| app_config.selected_model.clone());
        // An external default model's vision is unknown.
        path.as_deref()
            .filter(|p| {
                crate::config::Provider::split_target(p, &app_config.providers).is_none()
            })
            .map(|p| crate::server::find_mmproj_sibling(std::path::Path::new(p)).is_some())
    };

    // Housekeeping model for compaction; None falls back to the run's model.
    let utility_target = app_config
        .utility_target
        .as_deref()
        .and_then(|t| utility_target_client(&state, &app_config, t));
    let utility: Option<(&LlmClient, Option<String>)> =
        utility_target.as_ref().map(|(c, m)| (c, m.clone()));

    let gate = Arc::new(UiGate {
        app: app.clone(),
        runtime: runtime.clone(),
        question_event: if roleplay { "talk_question" } else { "harness_question" },
    });
    let steer_rt = runtime.clone();
    let steer: Arc<dyn Fn() -> Vec<String> + Send + Sync> =
        Arc::new(move || std::mem::take(&mut *steer_rt.steering.lock().unwrap()));

    let mut last_usage: Option<(u32, u32)> = None;
    let mut sink = |ev: StreamEvent| {
        if let StreamEvent::Usage { prompt_tokens, completion_tokens } = &ev {
            last_usage = Some((*prompt_tokens as u32, *completion_tokens as u32));
        }
        let _ = app.emit(event_name, serde_json::to_value(&ev).unwrap_or_default());
    };
    let app_events = app.clone();
    let mut event_sink = move |ev: AgentEvent| {
        let _ = app_events.emit(event_name, serde_json::to_value(&ev).unwrap_or_default());
    };

    // Roleplay sampling overrides ride the request; llama.cpp and most
    // OpenAI-compatible providers accept them per call.
    let sampling = if roleplay {
        let rp = &app_config.roleplay;
        let mut map = serde_json::Map::new();
        if let Some(t) = rp.temperature {
            map.insert("temperature".into(), serde_json::json!(t));
        }
        if let Some(p) = rp.top_p {
            map.insert("top_p".into(), serde_json::json!(p));
        }
        // llama.cpp extension; strict providers may reject unknown fields.
        if mode != crate::config::ServerMode::External {
            if let Some(r) = rp.repeat_penalty {
                map.insert("repeat_penalty".into(), serde_json::json!(r));
            }
        }
        (!map.is_empty()).then_some(serde_json::Value::Object(map))
    } else {
        None
    };

    let run = AgentRun {
        client,
        registry: Arc::new(registry),
        engine: runtime.engine.clone(),
        model: model.clone(),
        project: state.config.lock().unwrap().harness_active_project.clone(),
        // Roleplay keeps its effort in the profile config; the agent picks it
        // per send in the UI.
        reasoning_effort: if roleplay {
            app_config
                .roleplay
                .reasoning_effort
                .clone()
                .filter(|e| !e.trim().is_empty())
        } else {
            reasoning_effort.filter(|e| !e.is_empty())
        },
        sampling,
        max_turns,
        context_limit,
        verify_mode,
        compactions_log: Arc::new(Mutex::new(Vec::new())),
        utility,
        todos: Some(runtime.todos.clone()),
        subagents: Some(harness::agent::Subagents {
            jail,
            max_turns: subagent_max_turns,
            model: worker_model,
            exec_enabled: true,
            context_limit: worker_limit,
            client: worker_client,
            choices: subagent_choices,
            vision: subagent_vision,
        }),
    };

    let should_stop = runtime.abort.clone();
    let run_started = std::time::Instant::now();
    let base_head = if roleplay { None } else { git_head(&root) };
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
    *runtime.last_activity.lock().unwrap() = std::time::Instant::now();

    *runtime.history.lock().unwrap() = history;
    shift_meta_for_compaction(&runtime, &run.compactions_log.lock().unwrap().clone());
    // A failed run still persists its transcript before surfacing the error.
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(e) => {
            if let Err(e) = save_transcript_current(&state, &runtime, roleplay, None) {
                let _ = app.emit(
                    event_name,
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
    let footer_index = runtime
        .history
        .lock()
        .unwrap()
        .iter()
        .rposition(|m| m.role == "assistant");
    let changes = if roleplay {
        None
    } else {
        last_run_changes(&state, &root, base_head.as_deref())
    };
    if let Some(index) = footer_index {
        // Elapsed covers the whole run (prompt, every turn, tool calls), not
        // just the last generation the way the model outcome reports it.
        let total_ms = run_started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        runtime.meta.lock().unwrap().insert(
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
        if let Err(e) = save_transcript_current(&state, &runtime, roleplay, None) {
            let _ = app.emit(
                event_name,
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
    abort_runtime(&state.harness);
    Ok(())
}

/// Stop the running talk loop.
#[tauri::command]
#[specta::specta]
pub async fn talk_abort(state: State<'_, AppState>) -> Result<(), String> {
    abort_runtime(&state.talk);
    Ok(())
}

fn abort_runtime(runtime: &HarnessRuntime) {
    runtime.abort.store(true, Ordering::SeqCst);
    if let Some(tx) = runtime.pending.lock().unwrap().take() {
        let _ = tx.send(Approved::Denied);
    }
    if let Some(tx) = runtime.pending_question.lock().unwrap().take() {
        let _ = tx.send(QuestionAnswer::cancelled());
    }
    runtime.steering.lock().unwrap().clear();
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

const DISTILL_SUMMARY_SYSTEM: &str =
    "Summarize this session so its learnings survive after the context is cleared. Preserve: \
    the user's goals and outcomes, decisions and why, files created or changed, durable facts \
    or preferences, and open threads. Be concise; use bullets, one point per line.";

const DISTILL_COALESCE_SYSTEM: &str =
    "Rewrite this memory file. Keep every distinct fact, fold duplicates, preserve dates and \
    topic tags, and drop anything stale or trivial. Output only lines in the form \
    `- [YYYY-MM-DD] topic: text` (one per line, no headings, no commentary). Keep the result \
    under 4000 characters.";

/// Distill the session into memory, coalesce the memories, then start fresh.
#[tauri::command]
#[specta::specta]
pub async fn harness_distill(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    distill_impl(&state, app, &state.harness, false, "harness_event").await
}

/// Distill the talk thread into the character's memory, then start fresh.
#[tauri::command]
#[specta::specta]
pub async fn talk_distill(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    distill_impl(&state, app, &state.talk, true, "talk_event").await
}

async fn distill_impl(
    state: &AppState,
    app: AppHandle,
    runtime: &HarnessRuntime,
    roleplay: bool,
    event_name: &'static str,
) -> Result<String, String> {
    if runtime.running.load(Ordering::SeqCst) {
        return Err("Stop the running agent first".to_string());
    }
    let app_config = state.config.lock().unwrap().clone();
    let history = runtime.history.lock().unwrap().clone();
    let body = if history.first().map(|m| m.role.as_str()) == Some("system") {
        &history[1..]
    } else {
        &history[..]
    };
    if body.iter().all(|m| m.role != "user") {
        return Err("Nothing to distill yet — send a message first".to_string());
    }
    let transcript = harness::compact::render_transcript(body);
    if transcript.trim().is_empty() {
        return Err("Nothing to distill yet".to_string());
    }

    // Prefer the utility model when configured; else the same target rules as
    // a normal run (external provider, or the local server).
    let (client, model) = if let Some(resolved) = app_config
        .utility_target
        .as_deref()
        .and_then(|t| utility_target_client(&state, &app_config, t))
    {
        resolved
    } else if app_config.server_mode == crate::config::ServerMode::External {
        let target = app_config
            .external_target
            .clone()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| "External API mode: no model selected".to_string())?;
        let (p, m) = crate::config::Provider::split_target(&target, &app_config.providers)
            .ok_or_else(|| format!("External API mode: \"{target}\" has no configured provider"))?;
        (
            LlmClient::with_key(p.base_url.trim_end_matches('/').to_string(), p.api_key.clone()),
            Some(m.to_string()),
        )
    } else {
        let port = match state.server.lock().unwrap().status.clone() {
            crate::server::ServerStatus::Running { port, .. } => port,
            _ => return Err("Start the server first".to_string()),
        };
        let model = if app_config.server_mode == crate::config::ServerMode::Router {
            app_config
                .harness_roles
                .orchestrator
                .as_deref()
                .filter(|p| !p.trim().is_empty())
                .map(|p| {
                    let ids = crate::server::router_model_names(&router_role_entries(
                        &app_config.harness_roles,
                        &app_config.harness_role_params,
                        &app_config.providers,
                        None,
                        false,
                    ));
                    ids.get(p)
                        .cloned()
                        .unwrap_or_else(|| crate::server::file_stem_or_self(p))
                })
        } else {
            None
        };
        (server_client(port, &state), model)
    };

    runtime.abort.store(false, Ordering::SeqCst);
    let abort = runtime.abort.clone();
    let should_stop = move || abort.load(Ordering::SeqCst);
    let emit = |text: &str| {
        let _ = app.emit(
            event_name,
            serde_json::json!({"type": "distilled", "text": text}),
        );
    };

    emit("Distilling: summarizing the session…");
    let summary = harness::compact::summarize_chunked(
        &client,
        model.as_deref(),
        DISTILL_SUMMARY_SYSTEM,
        &transcript,
        &should_stop,
        |_| {},
    )
    .await
    .map_err(|e| e.to_string())?;
    if summary.is_empty() {
        return Err("Distillation produced an empty summary".to_string());
    }

    emit("Distilling: updating memory…");
    let (target, place) = if roleplay {
        (
            crate::roleplay::character_memory_path(
                state.config.lock().unwrap().roleplay.card_id.as_deref(),
            )
            .ok_or_else(|| "No memory location available".to_string())?,
            "character",
        )
    } else {
        let root = project_root(state).ok();
        let target = root
            .as_deref()
            .map(harness::memory::project_memory_path)
            .or_else(|| crate::config::data_dir().map(|d| d.join("werk").join("MEMORY.md")))
            .ok_or_else(|| "No memory location available".to_string())?;
        (target, if root.is_some() { "project" } else { "global" })
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut current = std::fs::read_to_string(&target).unwrap_or_default();
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    let mut appended = 0usize;
    for line in summary.lines() {
        let text = line.trim().trim_start_matches(['-', '*', '•', ' ']).trim();
        if text.is_empty() {
            continue;
        }
        let entry = harness::memory::entry_line("session", text);
        if current.len() + entry.len() > harness::memory::MEMORY_FILE_CAP {
            break;
        }
        current.push_str(&entry);
        appended += 1;
    }
    let backup = target.with_extension("md.bak");
    if appended > 0 {
        // Keep the previous file recoverable before the coalesce rewrites it.
        if target.is_file() {
            let _ = std::fs::copy(&target, &backup);
        }
        std::fs::write(&target, &current).map_err(|e| e.to_string())?;
    }

    emit("Distilling: coalescing memories…");
    let rewritten = harness::compact::summarize(
        &client,
        model.as_deref(),
        DISTILL_COALESCE_SYSTEM,
        &current,
        &should_stop,
        |_| {},
    )
    .await
    .unwrap_or_default();
    if !rewritten.trim().is_empty() {
        let tmp = target.with_extension("md.tmp");
        std::fs::write(&tmp, format!("{}\n", rewritten.trim())).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    }

    // Fresh session: the learnings are stored, the transcript can go.
    runtime.history.lock().unwrap().clear();
    runtime.todos.lock().unwrap().clear();
    truncate_meta(runtime, 0);
    *runtime.session_id.lock().unwrap() = None;
    runtime.steering.lock().unwrap().clear();

    let kept = if backup.is_file() { " (previous file kept as MEMORY.md.bak)" } else { "" };
    Ok(format!(
        "Distilled into {place} memory: {appended} entr{} written, memories coalesced{kept}. Started fresh.",
        if appended == 1 { "y" } else { "ies" },
    ))
}

/// Clear the task list (user action from the checkpoint strip).
#[tauri::command]
#[specta::specta]
pub async fn harness_todos_clear(state: State<'_, AppState>) -> Result<(), String> {
    state.harness.todos.lock().unwrap().clear();
    Ok(())
}

/// Clear the live transcript (keeps saved sessions).
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_reset(state: State<'_, AppState>) -> Result<(), String> {
    reset_runtime(&state.harness);
    Ok(())
}

fn reset_runtime(runtime: &HarnessRuntime) {
    runtime.history.lock().unwrap().clear();
    runtime.todos.lock().unwrap().clear();
    truncate_meta(runtime, 0);
    *runtime.session_id.lock().unwrap() = None;
    if let Some(tx) = runtime.pending.lock().unwrap().take() {
        let _ = tx.send(Approved::Denied);
    }
    if let Some(tx) = runtime.pending_question.lock().unwrap().take() {
        let _ = tx.send(QuestionAnswer::cancelled());
    }
    runtime.steering.lock().unwrap().clear();
}

/// Start a fresh roleplay chat seeded with the selected greeting.
#[tauri::command]
#[specta::specta]
pub async fn roleplay_start_chat(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    if state.talk.running.load(Ordering::SeqCst) {
        return Err("Stop the running talk first".to_string());
    }
    reset_runtime(&state.talk);
    let app_config = state.config.lock().unwrap().clone();
    let card = app_config
        .roleplay
        .card_id
        .as_deref()
        .and_then(|id| crate::roleplay::load_card(id).ok());
    let user_name = crate::roleplay::display_user_name(&app_config.roleplay);
    let Some(greeting) = card.as_ref().and_then(|c| {
        crate::roleplay::greeting(c, app_config.roleplay.greeting as usize, &user_name)
    }) else {
        return Ok(None);
    };
    {
        let mut history = state.talk.history.lock().unwrap();
        history.push(ChatMessage::assistant(greeting.clone()));
    }
    *state.talk.session_id.lock().unwrap() = Some(talk_session_id(&app_config));
    let history = state.talk.history.lock().unwrap().clone();
    let title = card.map(|c| c.name);
    save_transcript(&state, &state.talk, &history, title, true)
        .map_err(|e| format!("Session save failed: {e}"))?;
    Ok(Some(greeting))
}

/// Drop the last assistant response and the user turn that produced it.
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_rewind(state: State<'_, AppState>) -> Result<(), String> {
    rewind_runtime(&state, &state.harness, false)
}

/// Drop the last exchange from the talk thread.
#[tauri::command]
#[specta::specta]
pub async fn talk_rewind(state: State<'_, AppState>) -> Result<(), String> {
    rewind_runtime(&state, &state.talk, true)
}

fn rewind_runtime(
    state: &AppState,
    runtime: &HarnessRuntime,
    roleplay: bool,
) -> Result<(), String> {
    if runtime.running.load(Ordering::SeqCst) {
        return Err("Stop the running agent first".to_string());
    }
    let idx = {
        let mut history = runtime.history.lock().unwrap();
        let Some(idx) = history.iter().rposition(|m| m.role == "user") else {
            return Err("Nothing to rewind".to_string());
        };
        history.truncate(idx);
        idx
    };
    truncate_meta(runtime, idx);
    save_transcript_current(state, runtime, roleplay, None).map_err(|e| e.to_string())
}

/// Force-summarize older turns now; returns the folded message count.
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_compact(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<u32, String> {
    compact_runtime(&state, app, &state.harness, false, "harness_event").await
}

/// Force-summarize older turns in the talk thread.
#[tauri::command]
#[specta::specta]
pub async fn talk_compact(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<u32, String> {
    compact_runtime(&state, app, &state.talk, true, "talk_event").await
}

async fn compact_runtime(
    state: &AppState,
    app: AppHandle,
    runtime: &HarnessRuntime,
    roleplay: bool,
    event_name: &'static str,
) -> Result<u32, String> {
    if runtime.running.load(Ordering::SeqCst) {
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
    let client = server_client(port, state);
    let app_events = app.clone();
    let mut on_event = move |ev: AgentEvent| {
        let _ = app_events.emit(event_name, serde_json::to_value(&ev).unwrap_or_default());
    };
    let mut history = std::mem::take(&mut *runtime.history.lock().unwrap());
    let result = harness::compact::compact_history(
        &client,
        model.as_deref(),
        &mut history,
        limit,
        true,
        &|| runtime.abort.load(Ordering::SeqCst),
        &mut on_event,
    )
    .await;
    *runtime.history.lock().unwrap() = history;
    let info = result.map_err(|e| e.to_string())?.unwrap_or(harness::compact::CompactionInfo {
        cut: None,
        removed: 0,
    });
    if let Some(cut) = info.cut {
        shift_meta_for_compaction(runtime, &[cut]);
    }
    save_transcript_current(state, runtime, roleplay, None).map_err(|e| e.to_string())?;
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

/// Effective context for the orchestrator with no live slot, in the same
/// precedence the run uses: role override, the launch `--ctx-size` (the Run
/// page setting), then the GGUF header as a last resort. External mode uses
/// the provider's declared context instead.
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
    // The launch context is what the router children inherit and what a
    // single-model server was started with.
    let launch_n_ctx = state
        .server
        .lock()
        .unwrap()
        .config
        .as_ref()
        .map(|c| c.n_ctx)
        .unwrap_or(0);
    let effective = match mode {
        crate::config::ServerMode::External => {
            let target = external?;
            let config = state.config.lock().unwrap();
            crate::config::Provider::split_target(&target, &config.providers)
                .and_then(|(p, _)| p.context_length)
                .map(u64::from)
        }
        crate::config::ServerMode::Router => {
            // Mixed mode: an external role's window comes from its provider.
            let providers = state.config.lock().unwrap().providers.clone();
            if let Some((p, _)) = role_path
                .as_deref()
                .and_then(|path| crate::config::Provider::split_target(path, &providers))
            {
                return p.context_length;
            }
            let path = role_path.or_else(|| active_model_path(state));
            role_context_limit(role_ctx, launch_n_ctx, path.as_deref())
        }
        crate::config::ServerMode::Single => {
            let path = active_model_path(state);
            role_context_limit(None, launch_n_ctx, path.as_deref())
        }
    };
    effective.map(|n| n.min(u32::MAX as u64) as u32)
}

#[tauri::command]
#[specta::specta]
pub async fn harness_context_stats(
    state: State<'_, AppState>,
) -> Result<ContextStats, String> {
    context_stats(&state.harness, &state).await
}

/// Context usage for the roleplay/Talk transcript.
#[tauri::command]
#[specta::specta]
pub async fn talk_context_stats(
    state: State<'_, AppState>,
) -> Result<ContextStats, String> {
    context_stats(&state.talk, &state).await
}

async fn context_stats(
    runtime: &HarnessRuntime,
    state: &AppState,
) -> Result<ContextStats, String> {
    let used = harness::compact::estimate_tokens(&runtime.history.lock().unwrap());
    let mut stats = ContextStats {
        used: Some(used.min(u32::MAX as u64) as u32),
        total: None,
    };
    let mode = state.config.lock().unwrap().server_mode;
    // External API mode never reads the local server, even if one runs. A
    // router's own /slots and /props describe the router, not the loaded
    // child, so router mode computes the effective context instead.
    let read_live = mode == crate::config::ServerMode::Single;
    // Take the status by value: holding the guard in the match arms would
    // deadlock orchestrator_context, which locks the same mutex.
    let status = state.server.lock().unwrap().status.clone();
    let port = match status {
        crate::server::ServerStatus::Running { port, .. } if read_live => port,
        _ => {
            stats.total = orchestrator_context(state);
            return Ok(stats);
        }
    };
    let client = server_client(port, state);
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
        stats.total = orchestrator_context(state);
    }
    Ok(stats)
}

/// Current transcript for rebuilding the view (sessions, restarts).
#[tauri::command]
#[specta::specta]
pub async fn harness_agent_history(state: State<'_, AppState>) -> Result<HistoryView, String> {
    history_view(&state.harness)
}

/// Current talk transcript; lazily loads the active character's thread.
#[tauri::command]
#[specta::specta]
pub async fn talk_history(state: State<'_, AppState>) -> Result<HistoryView, String> {
    let app_config = state.config.lock().unwrap().clone();
    let empty = state.talk.history.lock().unwrap().is_empty()
        && state.talk.session_id.lock().unwrap().is_none();
    if empty {
        let id = talk_session_id(&app_config);
        if let Some(path) = transcript_path(&id, true) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(saved) = serde_json::from_str::<SessionFile>(&text) {
                    *state.talk.history.lock().unwrap() = saved.messages;
                    *state.talk.meta.lock().unwrap() = saved.meta;
                    *state.talk.session_id.lock().unwrap() = Some(saved.id);
                }
            }
        }
    }
    history_view(&state.talk)
}

fn history_view(runtime: &HarnessRuntime) -> Result<HistoryView, String> {
    let messages = runtime.history.lock().unwrap().clone();
    let meta = runtime
        .meta
        .lock()
        .unwrap()
        .iter()
        .map(|(index, meta)| MetaEntry { index: *index as u32, meta: meta.clone() })
        .collect();
    Ok(HistoryView {
        messages: messages.iter().map(to_history_message).collect(),
        meta,
        todos: todo_dtos(&runtime.todos.lock().unwrap()),
    })
}

/// Persist and clear the talk thread; used when the active card changes.
pub fn flush_talk(state: &AppState) {
    if state.talk.running.load(Ordering::SeqCst) {
        return;
    }
    if state.talk.history.lock().unwrap().is_empty() {
        reset_runtime(&state.talk);
        return;
    }
    let _ = save_transcript_current(state, &state.talk, true, None);
    reset_runtime(&state.talk);
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
    question_answer_impl(&state.harness, answer);
    Ok(())
}

/// Answer an ask_user question parked by the talk run.
#[tauri::command]
#[specta::specta]
pub async fn talk_question_answer(
    answer: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    question_answer_impl(&state.talk, answer);
    Ok(())
}

fn question_answer_impl(runtime: &HarnessRuntime, answer: String) {
    let sender = runtime.pending_question.lock().unwrap().take();
    let answer = answer.trim().to_string();
    if let Some(tx) = sender {
        let _ = tx.send(if answer.is_empty() {
            QuestionAnswer::cancelled()
        } else {
            QuestionAnswer { answer, cancelled: false }
        });
    }
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
    *state.harness.todos.lock().unwrap() = file.todos.clone();
    Ok(HistoryView {
        messages: file.messages.iter().map(to_history_message).collect(),
        meta: file
            .meta
            .into_iter()
            .map(|(index, meta)| MetaEntry { index: index as u32, meta })
            .collect(),
        todos: todo_dtos(&file.todos),
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
        state.harness.todos.lock().unwrap().clear();
        truncate_meta(&state.harness, 0);
        *state.harness.session_id.lock().unwrap() = None;
    }
    if let Some(path) = session_path(&id) {
        if path.exists() {
            // Parse before trashing: the in-memory copy powers Undo, the file
            // itself goes to the OS recycle bin instead of being erased.
            let stash = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<SessionFile>(&text).ok());
            trash::delete(&path)
                .map_err(|e| format!("Cannot move the session to the trash: {e}"))?;
            if let Some(file) = stash {
                *state.harness.deleted_session.lock().unwrap() = Some(DeletedSession {
                    file,
                    at: std::time::Instant::now(),
                });
            }
        }
    }
    Ok(())
}

/// Restore the most recently deleted session (in-memory copy, short window).
#[tauri::command]
#[specta::specta]
pub async fn harness_session_undo(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let Some(stash) = state.harness.deleted_session.lock().unwrap().take() else {
        return Ok(None);
    };
    if !undo_window_open(stash.at) {
        return Ok(None);
    }
    let id = stash.file.id.clone();
    write_session(&stash.file)?;
    Ok(Some(id))
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
        state.harness.todos.lock().unwrap().clear();
        truncate_meta(&state.harness, 0);
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
    state.harness.todos.lock().unwrap().clear();
    truncate_meta(&state.harness, 0);
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

/// Housekeeping model target (`provider:model`, `orchestrator`, `worker` or
/// none); next run.
#[tauri::command]
#[specta::specta]
pub async fn set_utility_target(
    target: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.utility_target = target.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    config.save().map_err(|e| e.to_string())
}

/// Reuse or create the HTTP client for an external provider.
fn ensure_external_client(
    external: &mut Vec<(String, LlmClient)>,
    provider: &crate::config::Provider,
) {
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

/// Owned client + model id for a target string: an external `provider:model`
/// or a local role name. None when empty, unknown, or the server is down.
fn utility_target_client(
    state: &AppState,
    app_config: &crate::config::AppConfig,
    target: &str,
) -> Option<(LlmClient, Option<String>)> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    if let Some((p, m)) = crate::config::Provider::split_target(target, &app_config.providers) {
        return Some((
            LlmClient::with_key(p.base_url.trim_end_matches('/').to_string(), p.api_key.clone()),
            Some(m.to_string()),
        ));
    }
    let role_path = match target {
        "orchestrator" => app_config.harness_roles.orchestrator.clone(),
        "worker" => app_config.harness_roles.worker.clone(),
        _ => return None,
    };
    // Mixed mode: an external role target runs on its provider.
    if let Some((p, m)) = role_path
        .as_deref()
        .and_then(|path| crate::config::Provider::split_target(path, &app_config.providers))
    {
        return Some((
            LlmClient::with_key(p.base_url.trim_end_matches('/').to_string(), p.api_key.clone()),
            Some(m.to_string()),
        ));
    }
    let port = match state.server.lock().unwrap().status.clone() {
        crate::server::ServerStatus::Running { port, .. } => port,
        _ => return None,
    };
    // Single mode serves the loaded model; only the router takes ids.
    let model = if app_config.server_mode == crate::config::ServerMode::Router {
        role_path.as_deref().filter(|p| !p.trim().is_empty()).map(|p| {
            let ids = crate::server::router_model_names(&router_role_entries(
                &app_config.harness_roles,
                &app_config.harness_role_params,
                &app_config.providers,
                None,
                false,
            ));
            ids.get(p)
                .cloned()
                .unwrap_or_else(|| crate::server::file_stem_or_self(p))
        })
    } else {
        None
    };
    Some((server_client(port, state), model))
}

/// Hide or show `AGENTS.md`/`.agent*` files for every agent tool.
#[tauri::command]
#[specta::specta]
pub async fn set_agent_files_hidden(hidden: bool, state: State<'_, AppState>) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.agent_files_hidden = hidden;
    config.save().map_err(|e| e.to_string())
}

/// Save the sensitive-file policy lists (extra patterns + allow exceptions).
#[tauri::command]
#[specta::specta]
pub async fn set_sensitive_shielding(
    patterns: Vec<String>,
    allow: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let clean = |v: Vec<String>| -> Vec<String> {
        v.into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    };
    let mut config = state.config.lock().unwrap();
    config.sensitive_patterns = clean(patterns);
    config.sensitive_allow = clean(allow);
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

/// Create a commented plugin template in the global plugins folder (the one
/// the Tools page lists); returns the manifest path so the UI can open it.
#[tauri::command]
#[specta::specta]
pub async fn scaffold_plugin() -> Result<String, String> {
    let base = crate::config::data_dir()
        .map(|d| d.join("werk").join("plugins"))
        .ok_or_else(|| "Cannot find the data directory".to_string())?;
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let mut name = "my_tool".to_string();
    let mut n = 1;
    while base.join(&name).exists() {
        n += 1;
        name = format!("my_tool-{n}");
    }
    let dir = base.join(&name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    let (script_name, script, command) = (
        "run.ps1",
        "# Werk plugin: stdin gets {\"args\": {...}}, stdout is the tool result.\n\
         # The command runs with the project directory as the working directory.\n\
         $payload = [Console]::In.ReadToEnd() | ConvertFrom-Json\n\
         $query = $payload.args.query\n\
         Write-Output \"replace me - got query: $query\"\n",
        "[\"powershell\", \"-NoProfile\", \"-File\", \"run.ps1\"]",
    );
    #[cfg(not(target_os = "windows"))]
    let (script_name, script, command) = (
        "run.sh",
        "#!/bin/sh\n\
         # Werk plugin: stdin gets {\"args\": {...}}, stdout is the tool result.\n\
         # The command runs with the project directory as the working directory.\n\
         echo \"replace me - stdin: $(cat)\"\n",
        "[\"sh\", \"run.sh\"]",
    );
    std::fs::write(dir.join(script_name), script).map_err(|e| e.to_string())?;

    let manifest = format!(
        "{{\n  \"name\": \"{name}\",\n  \"description\": \"One line the model sees when \
         choosing tools.\",\n  \"parameters\": {{\n    \"type\": \"object\",\n    \"properties\": \
         {{\n      \"query\": {{ \"type\": \"string\", \"description\": \"What to look up.\" }}\n    \
         }},\n    \"required\": [\"query\"]\n  }},\n  \"command\": {command},\n  \"approval\": \
         true\n}}\n"
    );
    let manifest_path = dir.join("plugin.json");
    std::fs::write(&manifest_path, manifest).map_err(|e| e.to_string())?;
    Ok(manifest_path.to_string_lossy().to_string())
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
    /// Pre-`/distill` copy, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
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
        "roleplay" => crate::roleplay::character_memory_path(
            state.config.lock().unwrap().roleplay.card_id.as_deref(),
        )
        .ok_or_else(|| "Cannot find data directory".to_string())?,
        _ => return Err("Unknown memory scope".to_string()),
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let backup = std::fs::read_to_string(path.with_extension("md.bak")).ok();
    Ok(MemoryFileDto {
        scope: scope.into(),
        path: path.to_string_lossy().to_string(),
        exists: path.exists(),
        text,
        backup,
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
        let all = PromptTools::all();
        let single = system_prompt_for(
            harness::agent::VerifyMode::Normal,
            crate::config::ServerMode::Single,
            all,
        );
        let external = system_prompt_for(
            harness::agent::VerifyMode::Normal,
            crate::config::ServerMode::External,
            all,
        );
        let router = system_prompt_for(
            harness::agent::VerifyMode::Normal,
            crate::config::ServerMode::Router,
            all,
        );
        assert!(!single.contains("subagent"), "{single}");
        assert!(!external.contains("subagent"), "{external}");
        assert!(router.contains("researcher subagent"), "{router}");
        // Verification off drops the paragraph entirely.
        let off = system_prompt_for(
            harness::agent::VerifyMode::Off,
            crate::config::ServerMode::Router,
            all,
        );
        assert!(!off.contains("Verify load-bearing"), "{off}");
    }

    #[test]
    fn context_precedence_role_then_launch_then_gguf() {
        // The role override wins over the launch ctx-size.
        assert_eq!(role_context_limit(Some(90_000), 32_768, None), Some(90_000));
        // The launch value is what the Run page sets; a zero override falls
        // through to it instead of jumping to the GGUF training maximum.
        assert_eq!(role_context_limit(Some(0), 92_160, None), Some(92_160));
        assert_eq!(role_context_limit(None, 92_160, None), Some(92_160));
        // Nothing configured and no readable model: unknown.
        assert_eq!(role_context_limit(None, 0, None), None);
    }

    #[test]
    fn subagent_block_names_choices_and_default() {
        let targets = vec![
            "NeoHorse-1-9B-Q8_0".to_string(),
            "LFM2.5-VL-3B-Uncensored-Q8_0".to_string(),
        ];
        let block =
            subagent_prompt_block(&targets, Some("LFM2.5-VL-3B-Uncensored-Q8_0"), Some(true));
        assert!(
            block.contains("Models: NeoHorse-1-9B-Q8_0, LFM2.5-VL-3B-Uncensored-Q8_0"),
            "{block}"
        );
        assert!(
            block.contains("the default is LFM2.5-VL-3B-Uncensored-Q8_0 (vision: yes)"),
            "{block}"
        );
        assert!(!block.contains("Subagents run on"), "{block}");
        // External: choices only; the run model is the default.
        let external = subagent_prompt_block(&targets, None, None);
        assert!(external.contains("the default is the main model"), "{external}");
        // No choices at all still states the default; unknown vision reads
        // as such rather than claiming "no".
        let none = subagent_prompt_block(&[], Some("provider:model"), None);
        assert!(none.contains("the default is provider:model (vision: unknown)"), "{none}");
        assert!(!none.contains("Models:"), "{none}");
        let local = subagent_prompt_block(&[], Some("worker"), Some(false));
        assert!(local.contains("the default is worker (vision: no)"), "{local}");
    }

    #[test]
    fn prompt_stays_compact() {
        let full = system_prompt_for(
            harness::agent::VerifyMode::Normal,
            crate::config::ServerMode::Router,
            PromptTools::all(),
        );
        assert!(full.len() < 1_700, "prompt grew to {} chars: {full}", full.len());
        for needle in ["file tools", "approval", "remember", "ask_user", "get_time", "Verify"] {
            assert!(full.contains(needle), "missing {needle}: {full}");
        }
    }

    #[test]
    fn disabled_tools_leave_the_prompt() {
        let disabled: Vec<String> = ["remember", "ask_user", "spawn_subagent", "get_time"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let prompt = system_prompt_for(
            harness::agent::VerifyMode::Normal,
            crate::config::ServerMode::Router,
            PromptTools::from_disabled(&disabled),
        );
        assert!(!prompt.contains("remember tool"), "{prompt}");
        assert!(!prompt.contains("ask_user"), "{prompt}");
        assert!(!prompt.contains("subagent"), "{prompt}");
        assert!(!prompt.contains("get_time"), "{prompt}");
        // Remaining tools stay named.
        assert!(prompt.contains("read_file"), "{prompt}");
        assert!(prompt.contains("exec"), "{prompt}");

        let all_off: Vec<String> = [
            "read_file",
            "write_file",
            "edit_file",
            "find_files",
            "search_content",
            "exec",
            "remember",
            "ask_user",
            "spawn_subagent",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let bare = system_prompt_for(
            harness::agent::VerifyMode::Normal,
            crate::config::ServerMode::Single,
            PromptTools::from_disabled(&all_off),
        );
        assert!(!bare.contains("read_file") && !bare.contains("exec"), "{bare}");
        assert!(bare.contains("sandboxed project directory"), "{bare}");
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
    fn undo_window_expires() {
        assert!(undo_window_open(std::time::Instant::now()));
        assert!(!undo_window_open(
            std::time::Instant::now()
                - SESSION_UNDO_WINDOW
                - std::time::Duration::from_secs(1)
        ));
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
