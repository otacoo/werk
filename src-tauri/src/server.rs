//! llama-server lifecycle: config, arg building, router presets, process.
//! Explicit user values always pass through; auto-sizing only fills gaps.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};

// ── Config ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ServerConfig {
    /// Empty means router mode (models come from the preset file).
    pub model_path: String,
    #[serde(default)]
    pub mmproj_path: Option<String>,
    pub host: String,
    pub port: u16,
    /// 0 = auto (omit --ctx-size).
    pub n_ctx: u32,
    /// -1 = all layers on GPU, 0 = CPU only.
    pub n_gpu_layers: i32,
    #[serde(default)]
    pub n_threads: Option<i32>,
    #[serde(default = "default_flash_attn")]
    pub flash_attn: String,
    #[serde(default = "default_cache_type")]
    pub cache_type_k: String,
    #[serde(default = "default_cache_type")]
    pub cache_type_v: String,
    pub temperature: f32,
    pub top_k: i32,
    pub min_p: f32,
    pub top_p: f32,
    /// -1 = unlimited.
    pub n_predict: i32,
    /// True = no sampling flags at all (server/harness defaults apply).
    #[serde(default)]
    pub disable_sampling: bool,
    /// 0 = omit (server default).
    pub n_batch: u32,
    /// 0 = omit (server default).
    pub n_ubatch: u32,
    pub cont_batching: bool,
    #[serde(default)]
    pub seed: Option<u32>,
    #[serde(default)]
    pub rope_freq_scale: Option<f32>,
    #[serde(default)]
    pub rope_freq_base: Option<f32>,
    pub parallel: u32,
    /// Child process working directory.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Extra CLI flags: key = flag without `--`, value = argument.
    /// `__raw__` holds free-form text appended verbatim.
    #[serde(default)]
    pub extra_params: HashMap<String, String>,
}

fn default_flash_attn() -> String {
    "auto".to_string()
}

fn default_cache_type() -> String {
    "f16".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            model_path: String::new(),
            mmproj_path: None,
            host: "127.0.0.1".to_string(),
            port: 8080,
            n_ctx: 0,
            n_gpu_layers: -1,
            n_threads: None,
            flash_attn: default_flash_attn(),
            cache_type_k: default_cache_type(),
            cache_type_v: default_cache_type(),
            temperature: 0.8,
            top_k: 40,
            min_p: 0.05,
            top_p: 0.95,
            n_predict: -1,
            disable_sampling: false,
            n_batch: 0,
            n_ubatch: 0,
            cont_batching: true,
            seed: None,
            rope_freq_scale: None,
            rope_freq_base: None,
            parallel: 1,
            working_dir: None,
            extra_params: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerStatus {
    Stopped,
    Starting,
    Running {
        port: u16,
        pid: u32,
        /// True once the process logs "listening on …" — before that the
        /// model is still loading.
        ready: bool,
    },
    Error { message: String },
}

// ── Sibling detection ─────────────────────────────────────────────────────

/// Same-dir `<stem>.jinja`, else `chat_template.jinja`.
pub fn find_jinja_sibling(model_path: &Path) -> Option<PathBuf> {
    let dir = model_path.parent()?;
    let stem = model_path.file_stem()?.to_string_lossy().to_string();
    if stem.is_empty() {
        return None;
    }
    let exact = format!("{stem}.jinja").to_lowercase();
    let mut fallback = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let lower = path.file_name()?.to_string_lossy().to_lowercase();
        if !lower.ends_with(".jinja") {
            continue;
        }
        if lower == exact {
            return Some(path);
        }
        if lower == "chat_template.jinja" {
            fallback = fallback.or(Some(path));
        }
    }
    fallback
}

/// Same-dir file with `mmproj` in the name (M1: filename only).
pub fn find_mmproj_sibling(model_path: &Path) -> Option<PathBuf> {
    let dir = model_path.parent()?;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let lower = path.file_name()?.to_string_lossy().to_lowercase();
        if lower.ends_with(".gguf") && lower.contains("mmproj") {
            return Some(path);
        }
    }
    None
}

/// Draft kind: purpose-built drafter vs separate MTP head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecDraftKind {
    Dspark,
    MtpHead,
}

/// Sibling draft file next to a model: a `dspark` file, else an `mtp-*` head.
pub fn find_spec_draft(model_path: &Path) -> Option<(PathBuf, SpecDraftKind)> {
    let dir = model_path.parent()?;
    let self_name = model_path.file_name()?.to_string_lossy().to_lowercase();
    let mut mtp = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let lower = path.file_name()?.to_string_lossy().to_lowercase();
        if lower == self_name || !lower.ends_with(".gguf") {
            continue;
        }
        if lower.contains("dspark") {
            return Some((path, SpecDraftKind::Dspark));
        }
        let stem = lower.strip_suffix(".gguf").unwrap_or(&lower);
        if mtp.is_none() && stem.split(['-', '_', ' ', '.']).next() == Some("mtp") {
            mtp = Some(path);
        }
    }
    mtp.map(|p| (p, SpecDraftKind::MtpHead))
}

/// Bytes of the companion files a launch also loads (mmproj, draft model).
pub fn companion_bytes(model_path: &str) -> u64 {
    let model = Path::new(model_path);
    let mut total = 0u64;
    if let Some(p) = find_mmproj_sibling(model) {
        total += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    }
    if let Some((p, _)) = find_spec_draft(model) {
        total += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    }
    total
}

/// True when the user set a template explicitly (beats auto-detection).
fn explicit_template(extra: &HashMap<String, String>) -> bool {
    extra.contains_key("chat-template-file") || extra.contains_key("chat-template")
}

// ── Arg builder ───────────────────────────────────────────────────────────

/// Argv + human-readable notes (auto-attaches, overrides). Pure function.
// ── Built-in tools ──────────────────────────────────────────────────────────
// Only the cross-build set below may reach `--tools`; unknown names fail
// server startup, so anything else is dropped.

const KNOWN_TOOLS: [&str; 7] = [
    "read_file",
    "file_glob_search",
    "grep_search",
    "exec_shell_command",
    "write_file",
    "edit_file",
    "get_info",
];

/// Canonical `--tools` value for a selection. `"all"` expands, a full set
/// collapses back to `"all"`, unknown names drop. None = omit the flag.
pub fn sanitize_tools(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return None;
    }
    if value.trim().eq_ignore_ascii_case("all") {
        return Some("all".to_string());
    }
    let mut sel: Vec<&str> = KNOWN_TOOLS
        .iter()
        .copied()
        .filter(|t| value.split(',').map(str::trim).any(|v| v == *t))
        .collect();
    if sel.is_empty() {
        return None;
    }
    sel.sort_unstable();
    if sel.len() == KNOWN_TOOLS.len() {
        return Some("all".to_string());
    }
    Some(sel.join(","))
}

pub fn is_known_tool(name: &str) -> bool {
    KNOWN_TOOLS.contains(&name)
}

pub fn tool_arg_value(tools: &[String]) -> Option<String> {
    sanitize_tools(&tools.join(","))
}

/// Apply the app-wide file-tool selection to a server config. Authoritative:
///
/// a stale `tools` value from a preset or session is overridden so startup
/// cannot abort on unsupported names.
pub fn apply_global_tools(config: &mut ServerConfig, tools: &[String]) {
    match tool_arg_value(tools) {
        Some(value) => {
            config.extra_params.insert("tools".to_string(), value);
        }
        None => {
            config.extra_params.remove("tools");
        }
    }
}

// ── Live server info ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RouterModelStatus {
    pub id: String,
    pub status: String,
}

/// Wire server info; counts fit u32 with room to spare.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ServerInfo {
    pub base_url: String,
    pub model_id: String,
    pub model_alias: String,
    pub model_path: String,
    pub n_ctx: u32,
    pub n_predict: i32,
    pub total_slots: u32,
    pub slots_idle: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<RouterModelStatus>,
    pub router_mode: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ServerToolInfo {
    pub name: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub description: String,
}

pub(crate) fn file_stem_or_self(s: &str) -> String {
    if s.contains('/') || s.contains('\\') {
        Path::new(s)
            .file_stem()
            .and_then(|x| x.to_str())
            .unwrap_or(s)
            .to_string()
    } else {
        s.to_string()
    }
}

/// Attach the launch API key when one is set (llama-server then requires it).
fn authed(req: reqwest::RequestBuilder, api_key: Option<&str>) -> reqwest::RequestBuilder {
    match api_key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => req.bearer_auth(key),
        None => req,
    }
}

pub async fn fetch_server_info(
    client: &reqwest::Client,
    port: u16,
    config: Option<&ServerConfig>,
    model_dirs: &[PathBuf],
) -> Result<ServerInfo> {
    let base = format!("http://127.0.0.1:{port}");
    let api_key = config
        .and_then(|c| c.extra_params.get("api-key").cloned())
        .filter(|k| !k.trim().is_empty());
    let router_mode = config.map(|c| c.model_path.is_empty()).unwrap_or(false);
    if router_mode {
        return fetch_router_info(client, &base, model_dirs, api_key.as_deref()).await;
    }
    let mut model_path = String::new();
    let mut model_alias = String::new();
    let mut total_slots: u32 = 1;
    if let Ok(resp) = authed(client.get(format!("{base}/props")), api_key.as_deref()).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            model_path = json.get("model_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
            model_alias = json.get("model_alias").and_then(|v| v.as_str()).unwrap_or("").to_string();
            total_slots = json.get("total_slots").and_then(|v| v.as_u64()).unwrap_or(1).min(u32::MAX as u64) as u32;
        }
    }
    let mut model_id = String::new();
    if let Ok(resp) = authed(client.get(format!("{base}/v1/models")), api_key.as_deref()).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            if let Some(first) = json.get("data").and_then(|v| v.as_array()).and_then(|a| a.first()) {
                model_id = first.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            }
        }
    }
    if model_id.is_empty() {
        model_id = model_alias.clone();
    }
    let mut n_ctx: u32 = 0;
    let mut n_predict: i32 = -1;
    let mut slots_idle: u32 = 0;
    if let Ok(resp) = authed(client.get(format!("{base}/slots")), api_key.as_deref()).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            if let Some(slots) = json.as_array() {
                if let Some(first) = slots.first() {
                    n_ctx = first.get("n_ctx").and_then(|v| v.as_u64()).unwrap_or(0).min(u32::MAX as u64) as u32;
                    n_predict = first.get("n_predict").and_then(|v| v.as_i64()).unwrap_or(-1).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                }
                slots_idle = slots
                    .iter()
                    .filter(|s| s.get("is_processing").and_then(|v| v.as_bool()).unwrap_or(true) == false)
                    .count()
                    .min(u32::MAX as usize) as u32;
            }
        }
    }
    Ok(ServerInfo {
        base_url: format!("{base}/v1"),
        model_id: file_stem_or_self(&model_id),
        model_alias: file_stem_or_self(&model_alias),
        model_path,
        n_ctx,
        n_predict,
        total_slots,
        slots_idle,
        api_key,
        models: Vec::new(),
        router_mode: false,
    })
}

/// Router info without triggering loads: registry ids plus the served
/// model's file and GGUF context (no proxied slot queries).
async fn fetch_router_info(
    client: &reqwest::Client,
    base: &str,
    model_dirs: &[PathBuf],
    api_key: Option<&str>,
) -> Result<ServerInfo> {
    let mut models = Vec::new();
    if let Ok(resp) = authed(client.get(format!("{base}/models")), api_key).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            let arr = if let Some(a) = json.as_array() {
                a.clone()
            } else {
                json.get("data").and_then(|v| v.as_array()).cloned().unwrap_or_default()
            };
            for m in arr {
                let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if id.is_empty() {
                    continue;
                }
                let status = m.get("status").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                models.push(RouterModelStatus { id, status });
            }
        }
    }
    let loaded = models.iter().find(|m| m.status == "loaded").or(models.first());
    let mut model_id = String::new();
    let mut model_path = String::new();
    let mut n_ctx: u32 = 0;
    if let Some(active) = loaded {
        model_id = active.id.clone();
        let installed = crate::models::list_installed_models(model_dirs);
        if let Some(m) = installed.iter().find(|m| {
            Path::new(&m.path).file_stem().and_then(|s| s.to_str()) == Some(active.id.as_str())
        }) {
            model_path = m.path.clone();
            n_ctx = m.context_length.map(|c| c.min(u32::MAX as u64) as u32).unwrap_or(0);
        }
    }
    Ok(ServerInfo {
        base_url: format!("{base}/v1"),
        model_id,
        model_alias: String::new(),
        model_path,
        n_ctx,
        n_predict: -1,
        total_slots: models.len().min(u32::MAX as usize) as u32,
        slots_idle: 0,
        api_key: api_key.map(str::to_string),
        models,
        router_mode: true,
    })
}

pub async fn fetch_server_tools(
    client: &reqwest::Client,
    port: u16,
    api_key: Option<&str>,
) -> Result<Vec<ServerToolInfo>> {
    let url = format!("http://127.0.0.1:{port}/tools");
    let resp = authed(client.get(&url), api_key)
        .send()
        .await
        .with_context(|| format!("Failed to fetch tools from {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("Server returned {} for {url}", resp.status());
    }
    let json: serde_json::Value = resp.json().await.context("Failed to parse /tools JSON")?;
    let arr = json
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("/tools did not return an array"))?;
    let mut out = Vec::new();
    for v in arr {
        let name = v.get("tool").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let display_name = v.get("display_name").and_then(|x| x.as_str()).unwrap_or(&name).to_string();
        let tool_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let description = v
            .get("definition")
            .and_then(|d| d.get("function"))
            .and_then(|f| f.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        out.push(ServerToolInfo { name, display_name, tool_type, description });
    }
    Ok(out)
}

pub fn build_args(config: &ServerConfig) -> (Vec<String>, Vec<String>) {
    let mut args = Vec::new();
    let mut notes = Vec::new();
    let router_mode = config.model_path.is_empty();

    if !router_mode {
        args.push("--model".to_string());
        args.push(config.model_path.clone());
    }
    if !router_mode {
        if let Some(ref mmproj) = config.mmproj_path {
            if !mmproj.is_empty() {
                args.push("--mmproj".to_string());
                args.push(mmproj.clone());
            }
        } else if let Some(sibling) = find_mmproj_sibling(Path::new(&config.model_path)) {
            // Vision models fail on image input without a projector.
            args.push("--mmproj".to_string());
            args.push(sibling.to_string_lossy().to_string());
            notes.push(format!("--mmproj auto-attached: {}", sibling.display()));
        }
    }
    // Upstream fit only adjusts *unset* args, so explicit values always
    // pass through; an explicit template wins over sibling auto-detection.
    if !router_mode
        && !explicit_template(&config.extra_params)
        && !config.extra_params.contains_key("no-chat-template")
    {
        if let Some(sibling) = find_jinja_sibling(Path::new(&config.model_path)) {
            args.push("--chat-template-file".to_string());
            args.push(sibling.to_string_lossy().to_string());
            notes.push(format!("--chat-template-file auto-attached: {}", sibling.display()));
        }
    }

    // Sibling draft file for speculative decoding (single-model only).
    if !router_mode
        && config.extra_params.contains_key("spec-draft")
        && !config.extra_params.contains_key("spec-draft-model")
    {
        if let Some((draft, kind)) = find_spec_draft(Path::new(&config.model_path)) {
            args.push("--spec-draft-model".to_string());
            args.push(draft.to_string_lossy().to_string());
            args.push("--spec-type".to_string());
            args.push(
                match kind {
                    SpecDraftKind::Dspark => "draft-dspark",
                    SpecDraftKind::MtpHead => "draft-mtp",
                }
                .to_string(),
            );
            notes.push(format!("--spec-draft-model auto-attached: {}", draft.display()));
        }
    }

    args.push("--host".to_string());
    args.push(config.host.clone());
    args.push("--port".to_string());
    args.push(config.port.to_string());

    if !router_mode {
        let fit = config.extra_params.get("fit").map(|s| s.as_str()).unwrap_or("on");
        let fit_on = fit != "off";
        // With fit on, llama.cpp sizes the offload itself; a user-set
        // --n-gpu-layers aborts that fit and invites VRAM overcommit.
        if fit_on {
            notes.push(
                "--fit on: GPU layers sized by llama.cpp (set Fit off to pin them)".to_string(),
            );
        } else {
            args.push("--n-gpu-layers".to_string());
            args.push(config.n_gpu_layers.to_string());
        }
        // Explicit ctx passes with fit on or off; 0 omits it (auto).
        if config.n_ctx > 0 {
            args.push("--ctx-size".to_string());
            args.push(config.n_ctx.to_string());
        }
        args.push("--fit".to_string());
        args.push(if fit_on { "on" } else { "off" }.to_string());
    }

    if let Some(threads) = config.n_threads {
        args.push("--threads".to_string());
        args.push(threads.to_string());
    }
    if config.flash_attn != "auto" {
        args.push("--flash-attn".to_string());
        args.push(config.flash_attn.clone());
    }
    if !config.cache_type_k.is_empty() {
        args.push("--cache-type-k".to_string());
        args.push(config.cache_type_k.clone());
    }
    if !config.cache_type_v.is_empty() {
        args.push("--cache-type-v".to_string());
        args.push(config.cache_type_v.clone());
    }
    if !config.disable_sampling {
        args.push("--temp".to_string());
        args.push(format!("{:.2}", config.temperature));
        args.push("--top-k".to_string());
        args.push(config.top_k.to_string());
        args.push("--min-p".to_string());
        args.push(format!("{:.4}", config.min_p));
        args.push("--top-p".to_string());
        args.push(format!("{:.4}", config.top_p));
    }
    if config.n_predict != -1 {
        args.push("--n-predict".to_string());
        args.push(config.n_predict.to_string());
    }
    if config.n_batch > 0 {
        args.push("--batch-size".to_string());
        args.push(config.n_batch.to_string());
    }
    if config.n_ubatch > 0 {
        args.push("--ubatch-size".to_string());
        args.push(config.n_ubatch.to_string());
    }
    if !config.cont_batching {
        args.push("--no-cont-batching".to_string());
    }
    if let Some(seed) = config.seed {
        args.push("--seed".to_string());
        args.push(seed.to_string());
    }
    if let Some(scale) = config.rope_freq_scale {
        args.push("--rope-freq-scale".to_string());
        args.push(format!("{:.6}", scale));
    }
    if let Some(base) = config.rope_freq_base {
        args.push("--rope-freq-base".to_string());
        args.push(format!("{:.1}", base));
    }
    args.push("--parallel".to_string());
    args.push(config.parallel.to_string());

    // Extra params, sorted for stable argv. `fit`/`__raw__` handled elsewhere.
    // Speculative-decoding extras are per-model and never ride into a router.
    let mut extra: Vec<_> = config
        .extra_params
        .iter()
        .filter(|(k, _)| {
            if matches!(
                k.as_str(),
                "fit" | "__raw__" | "no-chat-template" | "spec-draft"
            ) {
                return false;
            }
            // Server tools (--tools) are served by whichever process exposes
            // the API, so they apply to the router launch too.
            if router_mode && k.starts_with("spec-") {
                return false;
            }
            true
        })
        .collect();
    extra.sort_by_key(|(k, _)| (*k).clone());
    for (key, value) in extra {
        args.push(format!("--{key}"));
        if !value.is_empty() {
            args.push(value.clone());
        }
    }
    if let Some(raw) = config.extra_params.get("__raw__") {
        args.extend(raw.split_whitespace().map(str::to_string));
    }

    (args, notes)
}

/// One coherent launch suggestion from the model header + machine resources.
pub fn suggest_server_config(
    model_path: &str,
    model_size_mb: u64,
    total_vram_mb: u64,
    ram_mb: u64,
    cpu_cores: u32,
    cpu_threads: u32,
) -> ServerConfig {
    let meta = crate::models::read_model_metadata(Path::new(model_path));
    let spec = crate::estimate::ModelSpec::from_metadata(model_size_mb, meta.as_ref());
    let full = crate::estimate::suggest_full(&spec, total_vram_mb, ram_mb, cpu_cores, cpu_threads);
    ServerConfig {
        model_path: model_path.to_string(),
        n_ctx: full.n_ctx,
        n_gpu_layers: full.n_gpu_layers,
        n_threads: Some(full.n_threads),
        n_batch: full.n_batch,
        n_ubatch: full.n_ubatch,
        flash_attn: "auto".to_string(),
        cache_type_k: full.cache_type_k,
        cache_type_v: full.cache_type_v,
        ..ServerConfig::default()
    }
}

// ── Router preset ─────────────────────────────────────────────────────────

/// One preset section: a model plus optional per-model overrides, merged
/// over the router base args by llama-server.
#[derive(Debug, Clone, Default)]
pub struct PresetEntry {
    pub path: String,
    pub ctx_size: Option<u32>,
    pub n_gpu_layers: Option<i32>,
    pub chat_template_file: Option<String>,
    pub mmproj_path: Option<String>,
    pub draft_model: Option<String>,
    pub spec_type: Option<String>,
}

/// INI text for the given entries; same file repeats dedupe into one
/// section, first explicit override wins. Skips missing files.
pub fn render_router_preset(entries: &[PresetEntry]) -> String {
    let unique = unique_preset_entries(entries);
    let mut ini = String::new();
    for (i, name) in preset_section_names(&unique) {
        let entry = &unique[i];
        ini.push_str(&format!("[{}]\nmodel = {}\n", name, entry.path));
        if let Some(c) = entry.ctx_size {
            ini.push_str(&format!("ctx-size = {c}\n"));
        }
        if let Some(n) = entry.n_gpu_layers {
            ini.push_str(&format!("n-gpu-layers = {n}\n"));
        }
        if let Some(ref t) = entry.chat_template_file {
            ini.push_str(&format!("chat-template-file = {t}\n"));
        }
        if let Some(ref m) = entry.mmproj_path {
            ini.push_str(&format!("mmproj = {m}\n"));
        }
        if let Some(ref d) = entry.draft_model {
            ini.push_str(&format!("model-draft = {d}\n"));
        }
        if let Some(ref s) = entry.spec_type {
            ini.push_str(&format!("spec-type = {s}\n"));
        }
        ini.push('\n');
    }
    ini
}

/// Entries deduped by path and sorted like the preset file; first override
/// wins per field.
fn unique_preset_entries(entries: &[PresetEntry]) -> Vec<PresetEntry> {
    let mut unique: Vec<PresetEntry> = Vec::new();
    for e in entries {
        let path = e.path.trim();
        if path.is_empty() {
            continue;
        }
        match unique.iter_mut().find(|u| u.path == path) {
            Some(u) => {
                u.ctx_size = u.ctx_size.or(e.ctx_size);
                u.n_gpu_layers = u.n_gpu_layers.or(e.n_gpu_layers);
                u.chat_template_file =
                    u.chat_template_file.clone().or(e.chat_template_file.clone());
                u.mmproj_path = u.mmproj_path.clone().or(e.mmproj_path.clone());
                u.draft_model = u.draft_model.clone().or(e.draft_model.clone());
                u.spec_type = u.spec_type.clone().or(e.spec_type.clone());
            }
            None => unique.push(PresetEntry {
                path: path.to_string(),
                ctx_size: e.ctx_size,
                n_gpu_layers: e.n_gpu_layers,
                chat_template_file: e.chat_template_file.clone(),
                mmproj_path: e.mmproj_path.clone(),
                draft_model: e.draft_model.clone(),
                spec_type: e.spec_type.clone(),
            }),
        }
    }
    unique.sort_by(|a, b| a.path.cmp(&b.path));
    unique
}

/// Section names in entry order: file stem, `-2`/`-3` on duplicate stems.
/// Entries whose file is missing are skipped, as in the preset file.
fn preset_section_names(unique: &[PresetEntry]) -> Vec<(usize, String)> {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (i, entry) in unique.iter().enumerate() {
        if !Path::new(&entry.path).is_file() {
            continue;
        }
        let base = Path::new(&entry.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_string();
        let mut name = base.clone();
        let mut n = 1;
        while !used.insert(name.clone()) {
            n += 1;
            name = format!("{base}-{n}");
        }
        out.push((i, name));
    }
    out
}

/// Router model ids keyed by file path, matching the launched preset file.
pub fn router_model_names(entries: &[PresetEntry]) -> std::collections::HashMap<String, String> {
    let unique = unique_preset_entries(entries);
    preset_section_names(&unique)
        .into_iter()
        .map(|(i, name)| (unique[i].path.clone(), name))
        .collect()
}

/// Preset file for router mode (`--models-preset`); None when empty.
pub fn write_router_preset(dir: &Path, entries: &[PresetEntry]) -> Result<Option<PathBuf>> {
    let ini = render_router_preset(entries);
    if ini.trim().is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join("router_models.ini");
    std::fs::write(&path, ini)?;
    Ok(Some(path))
}

// ── Process ───────────────────────────────────────────────────────────────

pub struct ServerState {
    pub process: Option<Child>,
    pub status: ServerStatus,
    pub log_lines: Vec<String>,
    pub config: Option<ServerConfig>,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            process: None,
            status: ServerStatus::Stopped,
            log_lines: Vec::new(),
            config: None,
        }
    }
}

pub type SharedServerState = Arc<Mutex<ServerState>>;

pub fn new_server_state() -> SharedServerState {
    Arc::new(Mutex::new(ServerState::new()))
}

const LOG_CAP: usize = 500;

/// Start the server; every note and log line goes through `on_log` live
/// and lands in the ring buffer. `router_preset` adds `--models-preset`.
pub async fn start_server(
    server_binary: &Path,
    config: &ServerConfig,
    state: SharedServerState,
    router_preset: Option<&Path>,
    on_log: impl FnMut(String) + Send + 'static,
) -> Result<()> {
    {
        let mut s = state.lock().unwrap();
        if matches!(s.status, ServerStatus::Running { .. } | ServerStatus::Starting) {
            anyhow::bail!("Server is already running");
        }
        s.status = ServerStatus::Starting;
        s.log_lines.clear();
    }
    let (args, notes) = build_args(config);
    let mut argv = args;
    if let Some(preset) = router_preset {
        argv.push("--models-preset".to_string());
        argv.push(preset.to_string_lossy().to_string());
    }
    let on_log = Arc::new(Mutex::new(on_log));
    let emit = |state: &SharedServerState, line: String| {
        {
            let mut s = state.lock().unwrap();
            s.log_lines.push(line.clone());
            if s.log_lines.len() > LOG_CAP {
                let drain = s.log_lines.len() - LOG_CAP;
                s.log_lines.drain(0..drain);
            }
        }
        if let Ok(mut cb) = on_log.lock() {
            cb(line);
        }
    };
    for note in notes {
        emit(&state, note);
    }
    let mut cmd = Command::new(server_binary);
    cmd.args(&argv);
    if let Some(ref dir) = config.working_dir {
        if !dir.trim().is_empty() {
            cmd.current_dir(dir);
        }
    }
    crate::hidden::hide_tokio(&mut cmd);
    let mut child = match cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let mut s = state.lock().unwrap();
            s.status = ServerStatus::Error {
                message: format!("Failed to start {}: {e}", server_binary.display()),
            };
            anyhow::bail!("Failed to start {}: {e}", server_binary.display());
        }
    };
    let pid = child.id().unwrap_or(0);
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    {
        let mut s = state.lock().unwrap();
        s.process = Some(child);
        s.config = Some(config.clone());
        s.status = ServerStatus::Running { port: config.port, pid, ready: false };
    }
    emit(&state, format!("Server started (pid {pid})"));
    // Drain both pipes; lines land in the ring buffer and the callback.
    use tokio::io::AsyncRead;
    let mut streams: Vec<Box<dyn AsyncRead + Unpin + Send>> = Vec::new();
    if let Some(out) = stdout {
        streams.push(Box::new(out));
    }
    if let Some(err) = stderr {
        streams.push(Box::new(err));
    }
    for stream in streams {
        let state = state.clone();
        let on_log = on_log.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                {
                    let mut s = state.lock().unwrap();
                    s.log_lines.push(line.clone());
                    if s.log_lines.len() > LOG_CAP {
                        let drain = s.log_lines.len() - LOG_CAP;
                        s.log_lines.drain(0..drain);
                    }
                    // The listener socket opening marks the server ready;
                    // until then the model is still loading.
                    if line.to_lowercase().contains("listening on") {
                        if let ServerStatus::Running { port, pid, ready: false } = s.status {
                            s.status = ServerStatus::Running { port, pid, ready: true };
                        }
                    }
                }
                if let Ok(mut cb) = on_log.lock() {
                    cb(line);
                }
            }
        });
    }
    // Watch for the server exiting on its own so the UI can show the error
    // instead of a forever-loading state. `stop_server` clears `process`
    // first, which ends this watcher without touching the status.
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let exited: Option<Option<i32>> = {
                    let mut s = state.lock().unwrap();
                    match s.process.as_mut() {
                        Some(child) => match child.try_wait() {
                            Ok(Some(status)) => {
                                s.process = None;
                                s.config = None;
                                Some(status.code())
                            }
                            Ok(None) => None,
                            Err(_) => {
                                s.process = None;
                                s.config = None;
                                Some(None)
                            }
                        },
                        None => return,
                    }
                };
                let Some(code) = exited else { continue };
                let mut s = state.lock().unwrap();
                if matches!(
                    s.status,
                    ServerStatus::Starting | ServerStatus::Running { .. }
                ) {
                    let detail = code
                        .map(|c| format!("exit code {c}"))
                        .unwrap_or_else(|| "terminated".to_string());
                    let tail = s
                        .log_lines
                        .iter()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .map(|l| l.trim().chars().take(160).collect::<String>())
                        .unwrap_or_default();
                    s.status = ServerStatus::Error {
                        message: if tail.is_empty() {
                            format!("Server exited unexpectedly ({detail})")
                        } else {
                            format!("Server exited unexpectedly ({detail}): {tail}")
                        },
                    };
                }
                return;
            }
        });
    }
    Ok(())
}

/// Stop the server; no-op when already stopped.
pub async fn stop_server(state: &SharedServerState) -> Result<()> {
    let mut child = {
        let mut s = state.lock().unwrap();
        s.status = ServerStatus::Stopped;
        s.config = None;
        s.process.take()
    };
    if let Some(ref mut child) = child {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    Ok(())
}

/// Synchronous stop for process exit (no runtime needed).
pub fn kill_server_sync(state: &SharedServerState) {
    let mut s = match state.lock() {
        Ok(s) => s,
        Err(_) => return,
    };
    s.status = ServerStatus::Stopped;
    s.config = None;
    if let Some(mut child) = s.process.take() {
        // Best effort: SIGKILL equivalent without awaiting.
        child.start_kill().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_model() -> ServerConfig {
        ServerConfig { model_path: "/m.gguf".to_string(), ..Default::default() }
    }

    #[test]
    fn default_single_model_args() {
        let (args, _) = build_args(&single_model());
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"/m.gguf".to_string()));
        // Fit on + auto ctx: no --ctx-size, and no --n-gpu-layers (fit sizes it).
        assert!(!args.contains(&"--ctx-size".to_string()));
        assert!(!args.contains(&"--n-gpu-layers".to_string()));
        let fit = args.iter().position(|a| a == "--fit").unwrap();
        assert_eq!(args[fit + 1], "on");
    }

    #[test]
    fn explicit_ctx_passes_with_fit_on() {
        // Regression: fit only adjusts *unset* args, so an explicit ctx
        // must pass through instead of being silently dropped.
        let mut config = single_model();
        config.n_ctx = 122_880;
        let (args, _) = build_args(&config);
        let idx = args.iter().position(|a| a == "--ctx-size").expect("explicit ctx");
        assert_eq!(args[idx + 1], "122880");
        let fit = args.iter().position(|a| a == "--fit").unwrap();
        assert_eq!(args[fit + 1], "on");
    }

    #[test]
    fn fit_off_passes_ctx_and_flag() {
        let mut config = single_model();
        config.n_ctx = 8192;
        config.n_gpu_layers = 20;
        config.extra_params.insert("fit".to_string(), "off".to_string());
        let (args, _) = build_args(&config);
        let ctx = args.iter().position(|a| a == "--ctx-size").unwrap();
        assert_eq!(args[ctx + 1], "8192");
        let ngl = args.iter().position(|a| a == "--n-gpu-layers").unwrap();
        assert_eq!(args[ngl + 1], "20");
        let fit = args.iter().position(|a| a == "--fit").unwrap();
        assert_eq!(args[fit + 1], "off");
    }

    #[test]
    fn fit_on_drops_explicit_ngl() {
        let mut config = single_model();
        config.n_gpu_layers = 99;
        let (args, notes) = build_args(&config);
        assert!(!args.contains(&"--n-gpu-layers".to_string()));
        assert!(notes.iter().any(|n| n.contains("sized by llama.cpp")));
    }

    #[test]
    fn router_mode_omits_single_model_flags() {
        let (args, _) = build_args(&ServerConfig::default());
        assert!(!args.contains(&"--model".to_string()));
        assert!(!args.contains(&"--ctx-size".to_string()));
        assert!(!args.contains(&"--fit".to_string()));
        assert!(!args.contains(&"--n-gpu-layers".to_string()));
    }

    #[test]
    fn router_mode_drops_single_model_extras() {
        let mut config = ServerConfig {
            mmproj_path: Some("/m/mm.gguf".to_string()),
            ..Default::default()
        };
        config.extra_params.insert("spec-draft-model".to_string(), "/m/draft.gguf".to_string());
        config.extra_params.insert("ctx-size".to_string(), "4096".to_string());
        config.extra_params.insert("tools".to_string(), "read_file".to_string());
        let (args, _) = build_args(&config);
        assert!(!args.contains(&"--mmproj".to_string()));
        assert!(!args.contains(&"--spec-draft-model".to_string()));
        // Server tools are served by whichever process exposes the API, so the
        // router launch keeps them.
        let tools = args.iter().position(|a| a == "--tools").expect("router keeps --tools");
        assert_eq!(args[tools + 1], "read_file");
        // Shared server options still pass through.
        let ctx = args.iter().position(|a| a == "--ctx-size").expect("shared ctx");
        assert_eq!(args[ctx + 1], "4096");
    }

    #[test]
    fn explicit_template_beats_sibling_detection() {
        let dir = std::env::temp_dir().join(format!("werk-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("m.gguf");
        std::fs::write(&model, b"x").unwrap();
        std::fs::write(dir.join("m.jinja"), b"jinja").unwrap();
        // Auto-attach without override.
        let config = ServerConfig {
            model_path: model.to_string_lossy().to_string(),
            ..Default::default()
        };
        let (args, notes) = build_args(&config);
        let idx = args.iter().position(|a| a == "--chat-template-file").expect("auto");
        assert!(args[idx + 1].ends_with("m.jinja"));
        assert!(notes.iter().any(|n| n.contains("auto-attached")));
        // Explicit override wins, no auto-attach.
        let mut extra = HashMap::new();
        extra.insert("chat-template-file".to_string(), "/custom/t.jinja".to_string());
        let config = ServerConfig {
            model_path: model.to_string_lossy().to_string(),
            extra_params: extra,
            ..Default::default()
        };
        let (args, _) = build_args(&config);
        let hits: Vec<_> = args.iter().filter(|a| *a == "--chat-template-file").collect();
        assert_eq!(hits.len(), 1);
        let idx = args.iter().position(|a| a == "--chat-template-file").unwrap();
        assert_eq!(args[idx + 1], "/custom/t.jinja");
        // Sentinel suppresses auto-attach and never reaches argv.
        let mut extra = HashMap::new();
        extra.insert("no-chat-template".to_string(), String::new());
        let config = ServerConfig {
            model_path: model.to_string_lossy().to_string(),
            extra_params: extra,
            ..Default::default()
        };
        let (args, _) = build_args(&config);
        assert!(!args.iter().any(|a| a == "--chat-template-file"));
        assert!(!args.iter().any(|a| a == "--no-chat-template"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_draft_sibling_auto_attaches() {
        let dir = std::env::temp_dir().join(format!("werk-draft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("m.gguf");
        let draft = dir.join("m-DSpark.gguf");
        std::fs::write(&model, b"x").unwrap();
        std::fs::write(&draft, b"y").unwrap();
        let mut extra = HashMap::new();
        extra.insert("spec-draft".to_string(), String::new());
        let config = ServerConfig {
            model_path: model.to_string_lossy().to_string(),
            extra_params: extra,
            ..Default::default()
        };
        let (args, notes) = build_args(&config);
        let idx = args.iter().position(|a| a == "--spec-draft-model").expect("draft");
        assert!(args[idx + 1].ends_with("m-DSpark.gguf"));
        let idx = args.iter().position(|a| a == "--spec-type").expect("type");
        assert_eq!(args[idx + 1], "draft-dspark");
        assert!(notes.iter().any(|n| n.contains("spec-draft-model")));
        assert!(!args.iter().any(|a| a == "--spec-draft"));
        // Without the toggle the sibling is left alone.
        let config = ServerConfig {
            model_path: model.to_string_lossy().to_string(),
            ..Default::default()
        };
        let (args, _) = build_args(&config);
        assert!(!args.iter().any(|a| a == "--spec-draft-model"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn router_preset_ini_shape_and_dedup() {
        let dir = std::env::temp_dir().join(format!("werk-preset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("alpha.gguf");
        std::fs::write(&a, b"x").unwrap();
        let ini = render_router_preset(&[
            PresetEntry {
                path: a.to_string_lossy().to_string(),
                ctx_size: Some(32768),
                n_gpu_layers: Some(20),
                chat_template_file: None,
                ..Default::default()
            },
            PresetEntry {
                path: a.to_string_lossy().to_string(),
                ctx_size: None,
                n_gpu_layers: None,
                chat_template_file: Some("t.jinja".to_string()),
                ..Default::default()
            },
            PresetEntry { path: "missing.gguf".to_string(), ..Default::default() },
        ]);
        assert!(ini.contains("[alpha]"));
        assert!(!ini.contains("[alpha-2]"));
        assert!(ini.contains("ctx-size = 32768"));
        assert!(ini.contains("n-gpu-layers = 20"));
        assert!(ini.contains("chat-template-file = t.jinja"));
        assert!(!ini.contains("missing"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn router_model_names_match_preset_sections() {
        let dir = std::env::temp_dir().join(format!("werk-router-ids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let first = dir.join("a").join("model.gguf");
        let second = dir.join("b").join("model.gguf");
        std::fs::write(&first, b"x").unwrap();
        std::fs::write(&second, b"x").unwrap();
        let entries = vec![
            PresetEntry {
                path: second.to_string_lossy().to_string(),
                ..Default::default()
            },
            PresetEntry {
                path: first.to_string_lossy().to_string(),
                ..Default::default()
            },
            PresetEntry {
                path: dir.join("missing.gguf").to_string_lossy().to_string(),
                ..Default::default()
            },
        ];
        let names = router_model_names(&entries);
        let first = first.to_string_lossy().to_string();
        let second = second.to_string_lossy().to_string();
        // Sorted by path: a/model.gguf -> "model", b/model.gguf -> "model-2".
        assert_eq!(names.get(&first).map(String::as_str), Some("model"));
        assert_eq!(names.get(&second).map(String::as_str), Some("model-2"));
        assert_eq!(names.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_router_preset_round_trips_file() {
        let dir = std::env::temp_dir().join(format!("werk-preset-io-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("m.gguf");
        std::fs::write(&a, b"x").unwrap();
        let entries = vec![PresetEntry {
            path: a.to_string_lossy().to_string(),
            ..Default::default()
        }];
        let out = write_router_preset(&dir, &entries).unwrap().expect("file");
        assert_eq!(out, dir.join("router_models.ini"));
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(content.contains("[m]") && content.contains(&a.to_string_lossy().to_string()));
        assert!(write_router_preset(&dir, &[]).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jinja_sibling_prefers_stem_match() {
        let dir = std::env::temp_dir().join(format!("werk-jinja-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("m-Q4.gguf");
        std::fs::write(&model, b"x").unwrap();
        std::fs::write(dir.join("chat_template.jinja"), b"g").unwrap();
        std::fs::write(dir.join("m-Q4.jinja"), b"s").unwrap();
        assert_eq!(
            find_jinja_sibling(&model),
            Some(dir.join("m-Q4.jinja"))
        );
        std::fs::remove_file(dir.join("m-Q4.jinja")).unwrap();
        assert_eq!(
            find_jinja_sibling(&model),
            Some(dir.join("chat_template.jinja"))
        );
        std::fs::remove_file(dir.join("chat_template.jinja")).unwrap();
        assert_eq!(find_jinja_sibling(&model), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn start_failure_marks_error_status() {
        let state = new_server_state();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let probe = lines.clone();
        let err = start_server(
            Path::new("/nonexistent/llama-server"),
            &single_model(),
            state.clone(),
            None,
            move |line| probe.lock().unwrap().push(line),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("Failed to start"), "{err}");
        assert!(matches!(
            state.lock().unwrap().status,
            ServerStatus::Error { .. }
        ));
    }

    #[tokio::test]
    async fn stop_idle_server_is_noop() {
        let state = new_server_state();
        stop_server(&state).await.unwrap();
        assert_eq!(state.lock().unwrap().status, ServerStatus::Stopped);
        kill_server_sync(&state);
    }

    #[tokio::test]
    async fn second_start_while_starting_is_rejected() {
        let state = new_server_state();
        state.lock().unwrap().status = ServerStatus::Starting;
        let err = start_server(
            Path::new("/nonexistent/llama-server"),
            &single_model(),
            state.clone(),
            None,
            |_| {},
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("already running"), "{err}");
    }
}
