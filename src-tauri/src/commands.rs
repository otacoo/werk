//! Thin Tauri commands: validate, call drivers, map errors.
//! DTOs use JSON-safe integers (u32/i32/f64); domain math stays 64-bit.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::config::ActiveRuntime;
use crate::models::ModelInfo;
use crate::AppState;

// ── DTOs ──────────────────────────────────────────────────────────────────

/// Wire copy of a model (byte sizes as f64: exact to 9 PB in JSON).
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ModelDto {
    pub id: String,
    pub name: String,
    pub filename: String,
    pub path: String,
    pub size_bytes: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_b: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    pub is_vision: bool,
    pub is_reasoning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_repo: Option<String>,
}

fn to_model_dto(m: &ModelInfo) -> ModelDto {
    ModelDto {
        id: m.id.clone(),
        name: m.name.clone(),
        filename: m.filename.clone(),
        path: m.path.clone(),
        size_bytes: m.size_bytes as f64,
        quant: m.quant.clone(),
        params_b: m.params_b.clone(),
        context_length: m.context_length.map(|c| c.min(u32::MAX as u64) as u32),
        architecture: m.architecture.clone(),
        is_vision: m.is_vision,
        is_reasoning: m.is_reasoning,
        mmproj_path: m.mmproj_path.clone(),
        hf_repo: m.hf_repo.clone(),
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct CustomRuntimeDto {
    pub label: String,
    pub binary_path: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct RuntimeInfo {
    pub managed: Vec<crate::config::ManagedRuntime>,
    pub custom: Vec<CustomRuntimeDto>,
    pub active: ActiveRuntime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_binary: Option<String>,
    /// Runtimes root; managed builds live in `<base_dir>/<dir_name>`.
    pub base_dir: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ModelDirs {
    pub dirs: Vec<String>,
    pub download_dir: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct DownloadProgress {
    pub id: String,
    pub downloaded: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct GpuDto {
    pub name: String,
    pub vram_mb: u32,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct BackendDto {
    pub id: String,
    pub label: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Wire system info; RAM/VRAM are integers (exact in JSON under 2^53).
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SystemInfoDto {
    pub cpu_name: String,
    pub cpu_cores: u32,
    pub cpu_threads: u32,
    pub total_ram_mb: u32,
    pub available_ram_mb: u32,
    pub gpus: Vec<GpuDto>,
    pub os: String,
    pub arch: String,
    pub backends: Vec<BackendDto>,
    pub recommended_backend: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AssetDto {
    pub name: String,
    pub backend_id: String,
    pub backend_label: String,
    pub size_mb: f64,
    pub score: i32,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ReleaseDto {
    pub tag_name: String,
    pub build: u32,
    pub assets: Vec<AssetDto>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct FoundBinaryDto {
    pub binary_path: String,
    pub label: String,
}

/// Wire repo file; sizes ride f64 (exact to 9 PB in JSON).
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct HfFileDto {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<f64>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct KnownOwnerDto {
    pub id: String,
    pub description: String,
}

// ── Hardware ──────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn get_system_info() -> Result<SystemInfoDto, String> {
    // sysinfo + nvidia-smi/nvcc are blocking; keep them off the async pool.
    tauri::async_runtime::spawn_blocking(system_info_dto)
        .await
        .map_err(|e| format!("system info failed: {e}"))?
}

fn system_info_dto() -> Result<SystemInfoDto, String> {
    let system = crate::hardware::get_system_info().map_err(|e| e.to_string())?;
    let backends = crate::runtime::detect_backends(&system.gpus);
    let cuda = crate::hardware::nvidia_cuda_version();
    Ok(SystemInfoDto {
        cpu_name: system.cpu_name,
        cpu_cores: system.cpu_cores,
        cpu_threads: system.cpu_threads,
        total_ram_mb: system.total_ram_mb.min(u32::MAX as u64) as u32,
        available_ram_mb: system.available_ram_mb.min(u32::MAX as u64) as u32,
        gpus: system
            .gpus
            .into_iter()
            .map(|g| GpuDto { name: g.name, vram_mb: g.vram_mb.min(u32::MAX as u64) as u32 })
            .collect(),
        os: system.os,
        arch: system.arch,
        backends: backends
            .iter()
            .map(|b| BackendDto {
                id: b.id.clone(),
                label: b.label.clone(),
                available: b.available,
                version: if b.id == "cuda" { cuda.clone() } else { None },
            })
            .collect(),
        recommended_backend: crate::runtime::preferred_backend_id(&backends),
    })
}

// ── Config ────────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn set_wizard_completed(
    completed: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.wizard_completed = completed;
    config.save().map_err(|e| e.to_string())
}

// ── Server ────────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn get_server_status(state: State<'_, AppState>) -> Result<crate::server::ServerStatus, String> {
    Ok(state.server.lock().unwrap().status.clone())
}

#[tauri::command]
#[specta::specta]
pub async fn get_server_logs(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.server.lock().unwrap().log_lines.clone())
}

#[tauri::command]
#[specta::specta]
pub async fn stop_server(state: State<'_, AppState>) -> Result<(), String> {
    crate::server::stop_server(&state.server).await.map_err(|e| e.to_string())
}

/// Persisted single/router mode; takes effect on the next server start.
#[tauri::command]
#[specta::specta]
pub async fn set_server_mode(
    mode: crate::config::ServerMode,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.server_mode = mode;
    config.save().map_err(|e| e.to_string())
}

/// Replace the external OpenAI-compatible provider list.
#[tauri::command]
#[specta::specta]
pub async fn set_providers(
    providers: Vec<crate::config::Provider>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut normalized: Vec<crate::config::Provider> = Vec::new();
    for mut p in providers {
        p.id = p.id.trim().to_string();
        // Canonical API root: version path is appended per endpoint.
        p.base_url = harness::client::normalize_base(&p.base_url);
        p.api_key = p.api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        p.models = p
            .models
            .into_iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        if p.id.is_empty() || p.base_url.is_empty() {
            continue;
        }
        if p.id.contains(':') || p.id.contains(char::is_whitespace) {
            return Err(format!("Provider id must be a single word without ':' (got '{}')", p.id));
        }
        if normalized.iter().any(|x| x.id.eq_ignore_ascii_case(&p.id)) {
            return Err(format!("Duplicate provider id '{}'", p.id));
        }
        normalized.push(p);
    }
    let mut config = state.config.lock().unwrap();
    config.providers = normalized;
    config.save().map_err(|e| e.to_string())
}

/// Fetch the model ids a provider advertises (`GET /models`).
#[tauri::command]
#[specta::specta]
pub async fn test_provider(
    base_url: String,
    api_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let base = harness::client::normalize_base(&base_url);
    if base.is_empty() {
        return Err("Base URL is required".to_string());
    }
    let mut req = state.http_client.get(format!("{base}/v1/models"));
    if let Some(key) = api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "{base}/v1/models — {}",
            harness::client::http_error_detail(status.as_u16(), &text)
        ));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let arr = json
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| json.as_array())
        .cloned()
        .unwrap_or_default();
    let mut ids: Vec<String> = arr
        .iter()
        .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err("No models listed by the provider".to_string());
    }
    Ok(ids)
}

/// External API mode target (`provider-id:model-id`); empty clears it.
#[tauri::command]
#[specta::specta]
pub async fn set_external_target(
    target: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.external_target = target.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    config.save().map_err(|e| e.to_string())
}

/// Starred `provider-id:model-id` targets for the Mode picker.
#[tauri::command]
#[specta::specta]
pub async fn set_provider_favorites(
    favorites: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut normalized: Vec<String> = Vec::new();
    for f in favorites {
        let f = f.trim().to_string();
        if f.is_empty() || normalized.iter().any(|x| x == &f) {
            continue;
        }
        normalized.push(f);
    }
    let mut config = state.config.lock().unwrap();
    config.provider_favorites = normalized;
    config.save().map_err(|e| e.to_string())
}

/// Per-server MCP state for the Tools → Live view.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct McpServerStatus {
    pub name: String,
    pub running: bool,
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// MCP server status. `probe = false` only reports cached state (nothing is
/// started); `probe = true` starts the servers once and refreshes the tool
/// cache — the explicit "Check now" path.
#[tauri::command]
#[specta::specta]
pub async fn get_mcp_agent_tools(
    probe: bool,
    state: State<'_, AppState>,
) -> Result<Vec<McpServerStatus>, String> {
    let config = state.config.lock().unwrap().clone();
    let specs = crate::mcp::agent_specs(&config.mcp_disabled).map_err(|e| format!("{e:#}"))?;
    let manager = state.mcp_agent.manager(&specs, true);
    if !probe {
        return Ok(manager
            .cached_status()
            .into_iter()
            .map(to_status)
            .collect());
    }
    let worker = manager.clone();
    let statuses = tokio::task::spawn_blocking(move || worker.probe_status())
        .await
        .map_err(|_| "MCP discovery failed".to_string())?;
    state.mcp_agent.persist_cache(&manager);
    Ok(statuses.into_iter().map(to_status).collect())
}

fn to_status(s: harness::mcp::McpServerStatus) -> McpServerStatus {
    McpServerStatus {
        name: s.name,
        running: s.running,
        tools: s.tools,
        error: s.error,
    }
}

/// Router mode loads local GGUFs only; external endpoints live in External API mode.
fn reject_external_router_roles(app_config: &crate::config::AppConfig) -> Result<(), String> {
    for path in [
        app_config.harness_roles.orchestrator.as_deref(),
        app_config.harness_roles.worker.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if crate::config::Provider::split_target(path, &app_config.providers).is_some() {
            return Err(format!(
                "Router roles are local GGUF files — \"{path}\" is an external provider \
                 target. Use External API mode (Mode page) for external endpoints."
            ));
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn start_server(
    app: AppHandle,
    mut config: crate::server::ServerConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let app_config = state.config.lock().unwrap().clone();
    let mode = app_config.server_mode;
    if mode == crate::config::ServerMode::External {
        return Err(
            "External API mode does not run the local server — pick a provider and model on \
             the Mode page, then just open Chat"
                .to_string(),
        );
    }
    if mode == crate::config::ServerMode::Single && config.model_path.is_empty() {
        return Err("Single-model mode: select a model on the Run page first".to_string());
    }
    if mode == crate::config::ServerMode::Router
        && app_config.harness_roles.orchestrator.is_none()
        && app_config.harness_roles.worker.is_none()
    {
        return Err("Router mode: pick an orchestrator model on the Run page first".to_string());
    }
    if mode == crate::config::ServerMode::Router {
        reject_external_router_roles(&app_config)?;
    }
    // The mode owns the launch shape; single-model fields never leak into a
    // router launch.
    if mode == crate::config::ServerMode::Router {
        config.model_path.clear();
        config.mmproj_path = None;
    }
    // Server-native tools apply whenever a local server runs: whichever
    // process exposes the API (server or router) serves and executes them.
    crate::server::apply_global_tools(&mut config, &app_config.server_tools);
    // MCP servers attach at run time via a sidecar config.
    if let Ok(Some(mcp_path)) = crate::mcp::runtime_mcp_config_path(&app_config.mcp_disabled) {
        config.extra_params.insert(
            "mcp-servers-config".to_string(),
            mcp_path.to_string_lossy().to_string(),
        );
    }
    let binary = crate::runtime::server_binary(
        &app_config,
        &crate::runtime::runtimes_base_dir().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // Router preset from roles in router mode; single mode loads its model.
    let preset = if mode == crate::config::ServerMode::Router {
        let base_ctx = if config.n_ctx > 0 { Some(config.n_ctx) } else { None };
        let explicit = config.extra_params.contains_key("chat-template-file")
            || config.extra_params.contains_key("chat-template");
        let entries = crate::chat::router_role_entries(
            &app_config.harness_roles,
            &app_config.harness_role_params,
            base_ctx,
            explicit,
        );
        if entries.is_empty() {
            return Err("Router mode: pick an orchestrator model on the Run page first".to_string());
        }
        let dir = crate::config::data_dir()
            .map(|d| d.join("werk"))
            .ok_or_else(|| "Cannot find data directory".to_string())?;
        crate::server::write_router_preset(&dir, &entries).map_err(|e| e.to_string())?
    } else {
        None
    };
    crate::server::start_server(&binary, &config, state.server.clone(), preset.as_deref(), {
        let app = app.clone();
        move |line: String| {
            let _ = app.emit("server_log", &line);
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// Wire-ready launch command for the Run page (same prep as `start_server`).
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct LaunchPreview {
    pub binary: String,
    pub args: Vec<String>,
    pub notes: Vec<String>,
    pub attachments: Vec<AttachmentInfo>,
}

/// Auto-attached companion file; `auto` ones can be toggled off.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AttachmentInfo {
    pub kind: String,
    pub path: String,
    pub enabled: bool,
    pub auto: bool,
    /// Router role that owns it ("orchestrator"/"worker"); None = single model.
    #[serde(default)]
    pub role: Option<String>,
}

fn attachment_info(kind: &str, path: &str, enabled: bool, auto: bool, role: Option<&str>) -> AttachmentInfo {
    AttachmentInfo {
        kind: kind.to_string(),
        path: path.to_string(),
        enabled,
        auto,
        role: role.map(str::to_string),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn preview_server_args(
    mut config: crate::server::ServerConfig,
    state: State<'_, AppState>,
) -> Result<LaunchPreview, String> {
    let app_config = state.config.lock().unwrap().clone();
    if app_config.server_mode == crate::config::ServerMode::External {
        return Err(
            "External API mode does not run the local server — pick a provider and model on \
             the Mode page, then just open Chat"
                .to_string(),
        );
    }
    if app_config.server_mode == crate::config::ServerMode::Router {
        reject_external_router_roles(&app_config)?;
        config.model_path.clear();
        config.mmproj_path = None;
    }
    // Server-native tools apply in both local modes (see start_server).
    crate::server::apply_global_tools(&mut config, &app_config.server_tools);
    if let Ok(Some(mcp_path)) = crate::mcp::runtime_mcp_config_path(&app_config.mcp_disabled) {
        config.extra_params.insert(
            "mcp-servers-config".to_string(),
            mcp_path.to_string_lossy().to_string(),
        );
    }
    let binary = crate::runtime::runtimes_base_dir()
        .ok()
        .and_then(|base| crate::runtime::server_binary(&app_config, &base).ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "llama-server".to_string());
    let (mut args, notes) = crate::server::build_args(&config);

    // Companion files the launch will pick up (or not).
    let mut attachments = Vec::new();
    if !config.model_path.is_empty() {
        let model = std::path::Path::new(&config.model_path);
        let explicit_mm = config.mmproj_path.clone().unwrap_or_default();
        if !explicit_mm.is_empty() {
            attachments.push(attachment_info("mmproj", &explicit_mm, true, false, None));
        } else if let Some(sibling) = crate::server::find_mmproj_sibling(model) {
            let enabled = config.mmproj_path.is_none();
            attachments.push(attachment_info("mmproj", &sibling.to_string_lossy(), enabled, true, None));
        }
        let explicit_tpl = config
            .extra_params
            .get("chat-template-file")
            .or_else(|| config.extra_params.get("chat-template"))
            .cloned();
        if let Some(tpl) = explicit_tpl {
            attachments.push(attachment_info("chat-template", &tpl, true, false, None));
        } else if let Some(sibling) = crate::server::find_jinja_sibling(model) {
            let enabled = !config.extra_params.contains_key("no-chat-template");
            attachments.push(attachment_info("chat-template", &sibling.to_string_lossy(), enabled, true, None));
        }
        let explicit_draft = config.extra_params.get("spec-draft-model").cloned();
        if let Some(draft) = explicit_draft.filter(|d| !d.is_empty()) {
            attachments.push(attachment_info("draft model", &draft, true, false, None));
        } else if let Some((sibling, _kind)) = crate::server::find_spec_draft(model) {
            let enabled = config.extra_params.contains_key("spec-draft");
            attachments.push(attachment_info("draft model", &sibling.to_string_lossy(), enabled, true, None));
        }
    }

    // Router mode: role entries land in a preset file passed at launch.
    if config.model_path.is_empty() {
        let explicit = config.extra_params.contains_key("chat-template-file")
            || config.extra_params.contains_key("chat-template");
        let base_ctx = if config.n_ctx > 0 { Some(config.n_ctx) } else { None };
        let entries = crate::chat::router_role_entries(
            &app_config.harness_roles,
            &app_config.harness_role_params,
            base_ctx,
            explicit,
        );
        if !entries.is_empty() {
            if let Some(dir) = crate::config::data_dir().map(|d| d.join("werk")) {
                args.push("--models-preset".to_string());
                args.push(dir.join("router_models.ini").to_string_lossy().to_string());
            }
        }
        // Per-role companions travel in the preset file, not on argv.
        for (role, path, params) in [
            (
                "orchestrator",
                app_config.harness_roles.orchestrator.clone(),
                &app_config.harness_role_params.orchestrator,
            ),
            (
                "worker",
                app_config.harness_roles.worker.clone(),
                &app_config.harness_role_params.worker,
            ),
        ] {
            let Some(path) = path else { continue };
            let model = std::path::Path::new(&path);
            if let Some(tpl) = crate::server::find_jinja_sibling(model) {
                attachments.push(attachment_info(
                    "chat-template",
                    &tpl.to_string_lossy(),
                    !params.no_chat_template,
                    true,
                    Some(role),
                ));
            }
            if let Some(mm) = crate::server::find_mmproj_sibling(model) {
                attachments.push(attachment_info(
                    "mmproj",
                    &mm.to_string_lossy(),
                    !params.no_mmproj,
                    true,
                    Some(role),
                ));
            }
            if let Some((draft, _kind)) = crate::server::find_spec_draft(model) {
                attachments.push(attachment_info(
                    "draft model",
                    &draft.to_string_lossy(),
                    !params.no_draft,
                    true,
                    Some(role),
                ));
            }
        }
    }
    Ok(LaunchPreview { binary, args, notes, attachments })
}

// ── Project files ─────────────────────────────────────────────────────────

/// One entry in the Chat sidebar's project tree.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ProjectFileEntry {
    /// Forward-slash path relative to the project root.
    pub path: String,
    pub name: String,
    pub dir: bool,
    /// Changed on disk per `git status` (includes untracked files).
    pub changed: bool,
    /// Written or edited by the agent in the current session.
    pub touched: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ProjectFiles {
    pub root: String,
    pub git: bool,
    /// The scan hit its entry cap; the tree is incomplete.
    pub truncated: bool,
    pub entries: Vec<ProjectFileEntry>,
}

const FILE_TREE_MAX: usize = 4000;
const FILE_TREE_SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    ".gradle",
    ".idea",
];

/// Project folder tree with modification markers for the Chat sidebar.
#[tauri::command]
#[specta::specta]
pub async fn project_file_tree(state: State<'_, AppState>) -> Result<ProjectFiles, String> {
    let root = crate::chat::project_root(&state).map_err(|e| e.to_string())?;
    let touched = agent_touched_paths(&state);
    let git_changed = git_changed_paths(&root);
    let root_display = root.to_string_lossy().to_string();
    let git = git_changed.is_some();
    let changed_set = git_changed.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let (entries, truncated) = scan_project(&root, &changed_set, &touched);
        ProjectFiles { root: root_display, git, truncated, entries }
    })
    .await
    .map_err(|e| format!("project scan failed: {e}"))
}

/// Paths the agent wrote or edited in the current session.
fn agent_touched_paths(state: &State<'_, AppState>) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for msg in state.harness.history.lock().unwrap().iter() {
        let Some(calls) = msg.tool_calls.as_ref() else { continue };
        for call in calls {
            if call.function.name != "write_file" && call.function.name != "edit_file" {
                continue;
            }
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.function.arguments) {
                if let Some(path) = args.get("path").and_then(|p| p.as_str()) {
                    let norm = path.replace('\\', "/");
                    if !norm.is_empty() {
                        out.insert(norm);
                    }
                }
            }
        }
    }
    out
}

/// `git status --porcelain` relative paths, when the project is a repo.
fn git_changed_paths(root: &std::path::Path) -> Option<std::collections::HashSet<String>> {
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
    let mut set = std::collections::HashSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let mut path = line[3..].trim().to_string();
        // Quoted paths (spaces/unicode) come wrapped in double quotes.
        if path.len() >= 2 && path.starts_with('"') && path.ends_with('"') {
            path = path[1..path.len() - 1].to_string();
        }
        if !path.is_empty() {
            set.insert(path.replace('\\', "/"));
        }
    }
    Some(set)
}

/// Breadth-first project scan, dirs first, bounded by depth and count.
fn scan_project(
    root: &std::path::Path,
    changed: &std::collections::HashSet<String>,
    touched: &std::collections::HashSet<String>,
) -> (Vec<ProjectFileEntry>, bool) {
    let mut entries = Vec::new();
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, String, u32)> =
        std::collections::VecDeque::new();
    queue.push_back((root.to_path_buf(), String::new(), 0));
    let mut truncated = false;
    while let Some((dir, rel, depth)) = queue.pop_front() {
        if entries.len() >= FILE_TREE_MAX {
            truncated = true;
            break;
        }
        if depth > 12 {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut children: Vec<(std::path::PathBuf, String, bool)> = Vec::new();
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            let is_dir = item.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir && FILE_TREE_SKIP.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                continue;
            }
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            children.push((item.path(), child_rel, is_dir));
        }
        children.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        for (path, child_rel, is_dir) in children {
            if entries.len() >= FILE_TREE_MAX {
                truncated = true;
                break;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| child_rel.clone());
            let size = if is_dir {
                None
            } else {
                std::fs::metadata(&path)
                    .ok()
                    .map(|m| m.len().min(u32::MAX as u64) as u32)
            };
            entries.push(ProjectFileEntry {
                path: child_rel.clone(),
                name,
                dir: is_dir,
                changed: changed.contains(&child_rel),
                touched: touched.contains(&child_rel),
                size,
            });
            if is_dir {
                queue.push_back((path, child_rel, depth + 1));
            }
        }
    }
    (entries, truncated)
}

// ── Models ────────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn list_installed_models(state: State<'_, AppState>) -> Result<Vec<ModelDto>, String> {
    let dirs = state.config.lock().unwrap().all_model_dirs();
    // Directory walks can be slow; keep them off the async pool.
    tauri::async_runtime::spawn_blocking(move || {
        crate::models::list_installed_models(&dirs)
            .into_iter()
            .map(|m| to_model_dto(&m))
            .collect()
    })
    .await
    .map_err(|e| format!("model scan failed: {e}"))
}

#[tauri::command]
#[specta::specta]
pub async fn delete_model(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let roots = state.config.lock().unwrap().all_model_dirs();
    crate::models::delete_model(&PathBuf::from(path), &roots).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_selected_model(
    model_path: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.selected_model = model_path;
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_favorite_model(
    model_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    if let Some(pos) = config.favorite_models.iter().position(|id| id == &model_id) {
        config.favorite_models.remove(pos);
    } else {
        config.favorite_models.push(model_id);
    }
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_model_dirs(state: State<'_, AppState>) -> Result<ModelDirs, String> {
    let config = state.config.lock().unwrap();
    let dirs = config
        .all_model_dirs()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let download_dir = config
        .models_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(ModelDirs { dirs, download_dir })
}

#[tauri::command]
#[specta::specta]
pub async fn add_model_dir(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("Not a directory: {path}"));
    }
    let mut config = state.config.lock().unwrap();
    if !config.model_dirs.contains(&dir) {
        config.model_dirs.push(dir);
    }
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_download_dir(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let dir = PathBuf::from(&path);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut config = state.config.lock().unwrap();
    if !config.model_dirs.contains(&dir) {
        config.model_dirs.push(dir.clone());
    }
    config.download_dir = Some(dir);
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn remove_model_dir(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let dir = PathBuf::from(&path);
    let mut config = state.config.lock().unwrap();
    if config.model_dirs.len() <= 1 {
        return Err("Cannot remove the last model directory".to_string());
    }
    config.model_dirs.retain(|d| d != &dir);
    if config.download_dir.as_ref() == Some(&dir) {
        config.download_dir = None;
    }
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_hf_repo_files(
    repo_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<HfFileDto>, String> {
    let files =
        crate::models::hf_repo_files(&state.http_client, &repo_id).await.map_err(|e| e.to_string())?;
    Ok(files
        .into_iter()
        .map(|f| HfFileDto { path: f.path, size_bytes: f.size.map(|s| s as f64) })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn search_hf_models(
    query: String,
    owner: Option<String>,
    sort: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::HfModel>, String> {
    crate::models::search_hf(
        &state.http_client,
        &query,
        owner.as_deref(),
        sort.as_deref().unwrap_or("downloads"),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_known_owners(state: State<'_, AppState>) -> Result<Vec<KnownOwnerDto>, String> {
    let config = state.config.lock().unwrap();
    let mut ids = config.preferred_owners.clone();
    for (known, _) in crate::models::KNOWN_GGUF_OWNERS {
        if !ids.iter().any(|id| id == known) {
            ids.push(known.to_string());
        }
    }
    Ok(ids
        .into_iter()
        .map(|id| {
            let description = crate::models::KNOWN_GGUF_OWNERS
                .iter()
                .find(|(k, _)| *k == id)
                .map(|(_, d)| d.to_string())
                .unwrap_or_else(|| "User-added source".to_string());
            KnownOwnerDto { id, description }
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn validate_hf_owner(
    owner: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    crate::models::validate_hf_owner(&state.http_client, &owner)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_preferred_owners(
    owners: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    let mut seen = std::collections::HashSet::new();
    config.preferred_owners = owners
        .into_iter()
        .map(|o| o.trim().to_string())
        .filter(|o| !o.is_empty() && seen.insert(o.clone()))
        .collect();
    config.save().map_err(|e| e.to_string())
}

/// Download a repo file plus optional split parts under one progress id;
/// `save_as` renames the main file (companion sidecars). Returns the main file.
#[tauri::command]
#[specta::specta]
pub async fn download_model(
    app: AppHandle,
    repo_id: String,
    filename: String,
    split_parts: Option<Vec<String>>,
    save_as: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let dest_dir = state
        .config
        .lock()
        .unwrap()
        .models_dir()
        .map_err(|e| e.to_string())?;
    let id = filename.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    state.downloads.lock().unwrap().insert(id.clone(), cancel.clone());
    let app_events = app.clone();
    let id_events = id.clone();
    let done = std::sync::atomic::AtomicU64::new(0);
    let emit = |downloaded: u64, total: Option<u64>| {
        let _ = app_events.emit(
            "download_progress",
            DownloadProgress {
                id: id_events.clone(),
                downloaded: (downloaded + done.load(Ordering::SeqCst)) as f64,
                total: total.map(|t| (t + done.load(Ordering::SeqCst)) as f64),
            },
        );
    };
    let mut targets = vec![(filename.clone(), save_as)];
    if let Some(parts) = split_parts {
        targets.extend(parts.into_iter().map(|p| (p, None)));
    }
    let result: Result<PathBuf, String> = async {
        let mut main_dest: Option<PathBuf> = None;
        for (repo_file, rename) in &targets {
            let dest = crate::models::download_dest(&dest_dir, &repo_id, repo_file, rename.as_deref());
            if main_dest.is_none() {
                main_dest = Some(dest.clone());
            }
            let stop = cancel.clone();
            crate::models::download_hf_file_as(
                &state.dl_client,
                &repo_id,
                repo_file,
                &dest,
                &move || stop.load(Ordering::SeqCst),
                |downloaded, total| emit(downloaded, total),
            )
            .await
            .map_err(|e| e.to_string())?;
            let len = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            done.fetch_add(len, Ordering::SeqCst);
        }
        main_dest.ok_or_else(|| "Nothing downloaded".to_string())
    }
    .await;
    state.downloads.lock().unwrap().remove(&id);
    let dest = result?;
    crate::models::write_hf_sidecar(&dest, &repo_id);
    Ok(dest.to_string_lossy().to_string())
}

fn stop_flag(id: &str, state: &State<'_, AppState>) {
    if let Some(flag) = state.downloads.lock().unwrap().get(id) {
        flag.store(true, Ordering::SeqCst);
    }
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_download(id: String, state: State<'_, AppState>) -> Result<(), String> {
    stop_flag(&id, &state);
    Ok(())
}

/// Stop a download but keep the partial for resume.
#[tauri::command]
#[specta::specta]
pub async fn pause_download(id: String, state: State<'_, AppState>) -> Result<(), String> {
    stop_flag(&id, &state);
    Ok(())
}

/// Stop a download and discard its partial file (split parts included).
#[tauri::command]
#[specta::specta]
pub async fn discard_download(
    repo_id: String,
    filename: String,
    split_parts: Option<Vec<String>>,
    save_as: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if filename.contains("..") {
        return Err("Unsafe path".to_string());
    }
    stop_flag(&filename, &state);
    let dest_dir = state
        .config
        .lock()
        .unwrap()
        .models_dir()
        .map_err(|e| e.to_string())?;
    let dest = crate::models::download_dest(&dest_dir, &repo_id, &filename, save_as.as_deref());
    if dest.is_file() {
        std::fs::remove_file(&dest).map_err(|e| e.to_string())?;
    }
    for part in split_parts.unwrap_or_default() {
        if part.contains("..") {
            continue;
        }
        let dest = crate::models::download_dest(&dest_dir, &repo_id, &part, None);
        if dest.is_file() {
            let _ = std::fs::remove_file(&dest);
        }
    }
    state.downloads.lock().unwrap().remove(&filename);
    Ok(())
}

// ── Runtimes ──────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn get_runtime_info(state: State<'_, AppState>) -> Result<RuntimeInfo, String> {
    let config = state.config.lock().unwrap().clone();
    let base = crate::runtime::runtimes_base_dir().map_err(|e| e.to_string())?;
    let server_binary = crate::runtime::server_binary(&config, &base)
        .map(|p| p.to_string_lossy().to_string())
        .ok();
    Ok(RuntimeInfo {
        managed: config.managed_runtimes,
        custom: config
            .custom_runtimes
            .into_iter()
            .map(|c| CustomRuntimeDto {
                label: c.label,
                binary_path: c.binary_path.to_string_lossy().to_string(),
            })
            .collect(),
        active: config.active_runtime,
        server_binary,
        base_dir: base.to_string_lossy().to_string(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn download_runtime_latest(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let system =
        crate::hardware::get_system_info().map_err(|e| e.to_string())?;
    let backends = crate::runtime::detect_backends(&system.gpus);
    let cuda = crate::hardware::nvidia_cuda_version();
    let release = crate::runtime::fetch_latest_release(&state.http_client)
        .await
        .map_err(|e| e.to_string())?;
    let picked = crate::runtime::pick_asset(&release, &backends, cuda.as_deref())
        .ok_or_else(|| "No usable build for this host in the latest release".to_string())?;
    install_release_asset(
        &app,
        &state,
        &release.tag_name,
        picked.asset,
        &picked.backend_id,
        &picked.backend_label,
    )
    .await
}

/// Download one chosen asset from the latest release (wizard picker).
#[tauri::command]
#[specta::specta]
pub async fn download_runtime_asset(
    app: AppHandle,
    asset_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let system =
        crate::hardware::get_system_info().map_err(|e| e.to_string())?;
    let backends = crate::runtime::detect_backends(&system.gpus);
    let cuda = crate::hardware::nvidia_cuda_version();
    let release = crate::runtime::fetch_latest_release(&state.http_client)
        .await
        .map_err(|e| e.to_string())?;
    let picked = crate::runtime::score_release(&release, &backends, cuda.as_deref())
        .into_iter()
        .find(|s| s.asset.name == asset_name)
        .ok_or_else(|| format!("{asset_name} is not a usable build for this host"))?;
    install_release_asset(
        &app,
        &state,
        &release.tag_name,
        picked.asset,
        &picked.backend_id,
        &picked.backend_label,
    )
    .await
}

/// Usable assets in the latest release, best score first (wizard picker).
#[tauri::command]
#[specta::specta]
pub async fn check_release(state: State<'_, AppState>) -> Result<ReleaseDto, String> {
    let system =
        crate::hardware::get_system_info().map_err(|e| e.to_string())?;
    let backends = crate::runtime::detect_backends(&system.gpus);
    let cuda = crate::hardware::nvidia_cuda_version();
    let release = crate::runtime::fetch_latest_release(&state.http_client)
        .await
        .map_err(|e| e.to_string())?;
    let assets = crate::runtime::score_release(&release, &backends, cuda.as_deref())
        .into_iter()
        .map(|s| AssetDto {
            name: s.asset.name.clone(),
            backend_id: s.backend_id.clone(),
            backend_label: s.backend_label.clone(),
            size_mb: s.asset.size.unwrap_or(0) as f64 / 1048576.0,
            score: s.score.min(i32::MAX as i64) as i32,
        })
        .collect();
    Ok(ReleaseDto {
        tag_name: release.tag_name.clone(),
        build: crate::runtime::parse_build_tag(&release.tag_name).unwrap_or(0),
        assets,
    })
}

async fn install_release_asset(
    app: &AppHandle,
    state: &State<'_, AppState>,
    tag_name: &str,
    asset: &crate::runtime::ReleaseAsset,
    backend_id: &str,
    backend_label: &str,
) -> Result<(), String> {
    let build = crate::runtime::parse_build_tag(tag_name).unwrap_or(0);
    let id = format!("runtime:{}", asset.name);
    let cancel = Arc::new(AtomicBool::new(false));
    state.downloads.lock().unwrap().insert(id.clone(), cancel.clone());
    let app_events = app.clone();
    let id_events = id.clone();
    let base = crate::runtime::runtimes_base_dir().map_err(|e| e.to_string())?;
    let result: Result<(), String> = async {
        let dir = crate::runtime::install_asset(
            &state.dl_client,
            asset,
            backend_id,
            tag_name,
            &base,
            &move || cancel.load(Ordering::SeqCst),
            |downloaded, total| {
                let _ = app_events.emit(
                    "download_progress",
                    DownloadProgress {
                        id: id_events.clone(),
                        downloaded: downloaded as f64,
                        total: total.map(|t| t as f64),
                    },
                );
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let mut config = state.config.lock().unwrap();
        crate::runtime::register_downloaded_runtime(
            &mut config,
            build,
            tag_name,
            backend_id,
            backend_label,
            &asset.name,
            dir_name,
        );
        config.save().map_err(|e| e.to_string())
    }
    .await;
    state.downloads.lock().unwrap().remove(&id);
    result
}

/// Server binaries under a directory (existing-installation picker).
#[tauri::command]
#[specta::specta]
pub async fn scan_custom_binaries(path: String) -> Result<Vec<FoundBinaryDto>, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("Not a directory: {path}"));
    }
    Ok(crate::runtime::scan_binaries(&dir)
        .into_iter()
        .map(|b| FoundBinaryDto {
            binary_path: b.path.to_string_lossy().to_string(),
            label: b.label,
        })
        .collect())
}

/// Curated starter models with installed state.
#[tauri::command]
#[specta::specta]
pub async fn recommended_models(
    state: State<'_, AppState>,
) -> Result<Vec<crate::recommended::RecommendedDto>, String> {
    let dirs = state.config.lock().unwrap().all_model_dirs();
    Ok(crate::recommended::list_recommended(&dirs))
}

#[tauri::command]
#[specta::specta]
pub async fn set_active_runtime(
    selection: ActiveRuntime,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    match &selection {
        ActiveRuntime::Managed { build, backend_id } => {
            if !config
                .managed_runtimes
                .iter()
                .any(|r| r.build == *build && (backend_id.is_empty() || r.backend_id == *backend_id))
            {
                return Err("Runtime is not installed".to_string());
            }
        }
        ActiveRuntime::Custom { index } => {
            if config.custom_runtimes.get(*index as usize).is_none() {
                return Err("Runtime is not installed".to_string());
            }
        }
        ActiveRuntime::None => {}
    }
    config.active_runtime = selection;
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_managed_runtime(
    build: u32,
    backend_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let base = crate::runtime::runtimes_base_dir().map_err(|e| e.to_string())?;
    let mut config = state.config.lock().unwrap();
    crate::runtime::delete_managed_runtime(&mut config, build, &backend_id, &base)
        .map_err(|e| e.to_string())?;
    config.save().map_err(|e| e.to_string())
}

/// Drop older builds of the same backend when a new one finishes installing.
#[tauri::command]
#[specta::specta]
pub async fn set_auto_delete_runtimes(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.auto_delete_old_runtimes = enabled;
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn add_custom_runtime(
    path: String,
    state: State<'_, AppState>,
) -> Result<u32, String> {
    let binary = PathBuf::from(&path);
    if !binary.is_file() {
        return Err(format!("Not a file: {path}"));
    }
    let mut config = state.config.lock().unwrap();
    let index = crate::runtime::add_custom_runtime(&mut config, binary);
    config.save().map_err(|e| e.to_string())?;
    Ok(index)
}

#[tauri::command]
#[specta::specta]
pub async fn remove_custom_runtime(
    index: u32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    crate::runtime::remove_custom_runtime(&mut config, index).map_err(|e| e.to_string())?;
    config.save().map_err(|e| e.to_string())
}

// ── Presets ───────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn list_presets() -> Result<Vec<String>, String> {
    let dir = crate::presets::presets_dir().map_err(|e| e.to_string())?;
    Ok(crate::presets::list_presets(&dir))
}

#[tauri::command]
#[specta::specta]
pub async fn load_preset(name: String) -> Result<crate::server::ServerConfig, String> {
    let dir = crate::presets::presets_dir().map_err(|e| e.to_string())?;
    crate::presets::load_preset(&dir, &name).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn save_preset(
    name: String,
    config: crate::server::ServerConfig,
) -> Result<(), String> {
    let dir = crate::presets::presets_dir().map_err(|e| e.to_string())?;
    crate::presets::save_preset(&dir, &name, config).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_preset(name: String, state: State<'_, AppState>) -> Result<(), String> {
    let dir = crate::presets::presets_dir().map_err(|e| e.to_string())?;
    crate::presets::delete_preset(&dir, &name).map_err(|e| e.to_string())?;
    let mut config = state.config.lock().unwrap();
    config.model_presets.retain(|_, v| v != &name);
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_model_preset(
    model_path: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    Ok(state
        .config
        .lock()
        .unwrap()
        .model_presets
        .get(&model_path)
        .cloned())
}

#[tauri::command]
#[specta::specta]
pub async fn set_model_preset(
    model_path: String,
    preset: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    match preset {
        Some(name) => {
            config.model_presets.insert(model_path, name);
        }
        None => {
            config.model_presets.remove(&model_path);
        }
    }
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_last_preset(
    name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.last_preset = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_bench_visible(visible: bool, state: State<'_, AppState>) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.bench_visible = visible;
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_close_to_tray(
    app: AppHandle,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    {
        let mut config = state.config.lock().unwrap();
        config.close_to_tray = enabled;
        config.save().map_err(|e| e.to_string())?;
    }
    // The toggle owns the icon itself: no icon, no tray behavior.
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_visible(enabled).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_auto_check_updates(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.auto_check_updates = enabled;
    config.save().map_err(|e| e.to_string())
}

// ── Bench ─────────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn run_bench(
    model_path: String,
    n_gpu_layers: Option<i32>,
    state: State<'_, AppState>,
) -> Result<crate::bench::BenchResult, String> {
    let config = state.config.lock().unwrap().clone();
    let base = crate::runtime::runtimes_base_dir().map_err(|e| e.to_string())?;
    let binary = crate::bench::bench_binary(&config, &base).map_err(|e| e.to_string())?;
    crate::bench::run_quick_bench(&binary, &model_path, n_gpu_layers)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn bench_history() -> Result<Vec<crate::bench::BenchResult>, String> {
    Ok(crate::bench::load_history())
}

#[tauri::command]
#[specta::specta]
pub async fn clear_bench_history() -> Result<(), String> {
    crate::bench::clear_history().map_err(|e| e.to_string())
}

// ── Server tools + MCP ────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn set_tools(tools: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    let mut seen = std::collections::HashSet::new();
    config.server_tools = tools
        .into_iter()
        .filter(|t| crate::server::is_known_tool(t))
        .filter(|t| seen.insert(t.clone()))
        .collect();
    config.server_tools.sort();
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn list_mcp_servers(state: State<'_, AppState>) -> Result<crate::mcp::McpInfo, String> {
    let path = crate::mcp::mcp_config_path().map_err(|e| e.to_string())?;
    crate::mcp::ensure_defaults(&path).map_err(|e| e.to_string())?;
    let cfg = crate::mcp::load_at(&path).map_err(|e| e.to_string())?;
    let disabled = state.config.lock().unwrap().mcp_disabled.clone();
    let mut servers = crate::mcp::entries_from_config(&cfg);
    for s in servers.iter_mut() {
        s.enabled = !disabled.iter().any(|d| d == &s.name);
    }
    Ok(crate::mcp::McpInfo {
        path: path.to_string_lossy().to_string(),
        servers,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn save_mcp_servers(
    servers: Vec<crate::mcp::McpServerEntry>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let path = crate::mcp::mcp_config_path().map_err(|e| e.to_string())?;
    crate::mcp::save_at(&servers, &path).map_err(|e| e.to_string())?;
    // Config changed: the next run respawns harness-side MCP servers.
    state.mcp_agent.shutdown();
    // Disabled names live in AppConfig so mcp.json stays Cursor-compatible.
    let mut config = state.config.lock().unwrap();
    config.mcp_disabled = servers
        .iter()
        .filter(|s| !s.enabled)
        .map(|s| s.name.clone())
        .collect();
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_server_tools(state: State<'_, AppState>) -> Result<Vec<crate::server::ServerToolInfo>, String> {
    let (port, api_key) = {
        let s = state.server.lock().unwrap();
        let port = match &s.status {
            crate::server::ServerStatus::Running { port, .. } => *port,
            crate::server::ServerStatus::Starting => {
                if let Some(cfg) = &s.config {
                    cfg.port
                } else {
                    return Err("Server is starting, no port yet".to_string());
                }
            }
            _ => return Err("Server is not running".to_string()),
        };
        let api_key = s
            .config
            .as_ref()
            .and_then(|c| c.extra_params.get("api-key").cloned())
            .filter(|k| !k.trim().is_empty());
        (port, api_key)
    };
    crate::server::fetch_server_tools(&state.http_client, port, api_key.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_server_info(state: State<'_, AppState>) -> Result<crate::server::ServerInfo, String> {
    let (port, config) = {
        let s = state.server.lock().unwrap();
        match &s.status {
            crate::server::ServerStatus::Running { port, .. } => (*port, s.config.clone()),
            _ => return Err("Server is not running".to_string()),
        }
    };
    let dirs = state.config.lock().unwrap().all_model_dirs();
    crate::server::fetch_server_info(&state.http_client, port, config.as_ref(), &dirs)
        .await
        .map_err(|e| e.to_string())
}

// ── Estimate ──────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn estimate_memory(
    model_path: String,
    n_ctx: u32,
    cache_type_k: String,
    cache_type_v: String,
    n_gpu_layers: i32,
    fit: bool,
    state: State<'_, AppState>,
) -> Result<crate::estimate::MemoryEstimate, String> {
    let system =
        crate::hardware::get_system_info().map_err(|e| e.to_string())?;
    let meta = crate::models::read_model_metadata(PathBuf::from(&model_path).as_path())
        .ok_or_else(|| "Cannot read model metadata".to_string())?;
    let vram_total: u64 = system.gpus.iter().map(|g| g.vram_mb).sum();
    let model = crate::models::list_installed_models(&state.config.lock().unwrap().all_model_dirs())
        .into_iter()
        .find(|m| m.path == model_path);
    let size_mb = model.map(|m| m.size_bytes / 1024 / 1024).unwrap_or_else(|| {
        std::fs::metadata(&model_path).map(|m| m.len() / 1024 / 1024).unwrap_or(0)
    }) + crate::server::companion_bytes(&model_path) / 1024 / 1024;
    Ok(crate::estimate::estimate_memory(
        &meta,
        size_mb,
        n_ctx,
        &cache_type_k,
        &cache_type_v,
        n_gpu_layers,
        fit,
        vram_total,
        system.available_ram_mb,
    ))
}

/// Hardware-aware launch suggestion from the model header (auto-estimate).
#[tauri::command]
#[specta::specta]
pub async fn suggest_server_config(
    model_path: String,
    model_size_mb: u32,
) -> Result<crate::server::ServerConfig, String> {
    let system = crate::hardware::get_system_info().map_err(|e| e.to_string())?;
    let vram_total: u64 = system.gpus.iter().map(|g| g.vram_mb).sum();
    Ok(crate::server::suggest_server_config(
        &model_path,
        model_size_mb as u64 + crate::server::companion_bytes(&model_path) / 1024 / 1024,
        vram_total,
        system.available_ram_mb,
        system.cpu_cores,
        system.cpu_threads,
    ))
}

/// Size-only suggestion (no header read); notes for the estimate card.
#[tauri::command]
#[specta::specta]
pub async fn suggest_model_config(
    model_size_mb: u32,
) -> Result<crate::estimate::SuggestedConfig, String> {
    let system = crate::hardware::get_system_info().map_err(|e| e.to_string())?;
    let vram_total: u64 = system.gpus.iter().map(|g| g.vram_mb).sum();
    Ok(crate::estimate::suggest_config(
        model_size_mb as u64,
        vram_total,
        system.available_ram_mb,
        system.cpu_cores,
        system.cpu_threads,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn project_scan_skips_heavy_dirs_and_marks_changes() {
        let root = std::env::temp_dir().join(format!("werk-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("README.md"), b"hi").unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), b"x").unwrap();

        let changed: HashSet<String> = ["src/main.rs".to_string()].into();
        let touched: HashSet<String> = ["README.md".to_string()].into();
        let (entries, truncated) = scan_project(&root, &changed, &touched);

        assert!(!truncated);
        assert!(entries.iter().all(|e| !e.path.contains("node_modules")));
        assert!(entries.iter().any(|e| e.path == "src" && e.dir));
        assert!(entries
            .iter()
            .any(|e| e.path == "src/main.rs" && e.changed && !e.touched));
        assert!(entries
            .iter()
            .any(|e| e.path == "README.md" && e.touched && !e.changed));
        let _ = std::fs::remove_dir_all(&root);
    }
}
