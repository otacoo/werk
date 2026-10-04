//! GGUF models: header parsing, directory scan, HuggingFace downloads.
//! Filenames carry quant and family info; the header carries dimensions.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ── Metadata ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ModelMetadata {
    pub architecture: Option<String>,
    pub context_length: Option<u64>,
    pub embedding_length: Option<u64>,
    pub block_count: Option<u64>,
    pub attention_head_count: Option<u64>,
    pub attention_head_count_kv: Option<u64>,
    pub attention_key_length: Option<u64>,
    pub parameter_count: Option<u64>,
    /// `<arch>.expert_count`; MoE when > 0.
    pub expert_count: Option<u64>,
    pub size_label: Option<String>,
    pub chat_template: Option<String>,
    pub file_size: u64,
    /// `<arch>.target_layers` array length: EAGLE3 draft models declare it.
    pub target_layers: Option<u64>,
    /// A `*.nextn.eh_proj.weight` tensor exists: an MTP draft head.
    pub has_nextn: bool,
}

/// Template text beyond this is truncated (reasoning sniffing only).
const TEMPLATE_CAP: usize = 32_768;

/// GGUF value types we understand; others are skipped by width.
fn read_u32(r: &mut impl Read) -> Result<u32> {    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64(r: &mut impl Read) -> Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn read_gguf_string(r: &mut impl Read, cap: usize) -> Result<String> {
    let len = read_u64(r)? as usize;
    if len > cap {
        anyhow::bail!("String too long ({len} bytes)");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn skip_value(r: &mut (impl Read + std::io::Seek), ty: u32) -> Result<()> {
    match ty {
        // Fixed widths: u8 i8 bool u16 i32 u32 f32 u64 i64 f64.
        0 | 1 | 7 => {
            let mut b = [0u8; 1];
            r.read_exact(&mut b)?;
            Ok(())
        }
        2 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b)?;
            Ok(())
        }
        3 | 4 | 5 | 6 => {
            let mut b = [0u8; 4];
            r.read_exact(&mut b)?;
            Ok(())
        }
        10 | 11 | 12 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            Ok(())
        }
        // Anything else would misalign the stream: fail the parse so the
        // file is skipped instead of misread.
        _ => anyhow::bail!("Unknown GGUF type {ty}"),
    }
}

/// Read the GGUF header + metadata KVs; None on any problem (never fatal).
pub fn read_model_metadata(path: &Path) -> Option<ModelMetadata> {
    let file_size = std::fs::metadata(path).ok()?.len();
    let file = std::fs::File::open(path).ok()?;
    let mut r = std::io::BufReader::new(file);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }
    let version = read_u32(&mut r).ok()?;
    let n_tensors = read_u64(&mut r).ok()?;
    let n_kv = read_u64(&mut r).ok()?;
    if n_kv > 100_000 {
        return None;
    }
    let mut meta = ModelMetadata { file_size, ..Default::default() };
    for _ in 0..n_kv {
        let key = read_gguf_string(&mut r, 1024).ok()?;
        let ty = read_u32(&mut r).ok()?;
        // Suffix match: the arch prefix varies per model family.
        let leaf = key.rsplit('.').next().unwrap_or(&key);
        match (leaf, ty) {
            ("context_length", 4) => meta.context_length = read_u32(&mut r).ok().map(|v| v as u64),
            ("embedding_length", 4) => meta.embedding_length = read_u32(&mut r).ok().map(|v| v as u64),
            ("block_count", 4) => meta.block_count = read_u32(&mut r).ok().map(|v| v as u64),
            ("head_count", 4) => meta.attention_head_count = read_u32(&mut r).ok().map(|v| v as u64),
            ("head_count_kv", 4) => meta.attention_head_count_kv = read_u32(&mut r).ok().map(|v| v as u64),
            ("key_length", 4) => meta.attention_key_length = read_u32(&mut r).ok().map(|v| v as u64),
            ("parameter_count", 10) => meta.parameter_count = read_u64(&mut r).ok(),
            ("expert_count", 4) => meta.expert_count = read_u32(&mut r).ok().map(|v| v as u64),
            ("size_label", 8) => meta.size_label = read_gguf_string(&mut r, 256).ok(),
            ("architecture", 8) => meta.architecture = read_gguf_string(&mut r, 256).ok(),
            ("chat_template", 8) => {
                meta.chat_template = read_gguf_string(&mut r, TEMPLATE_CAP).ok()
            }
            ("chat_template", _) => {
                skip_value(&mut r, ty).ok()?;
            }
            (_, 8) => {
                read_gguf_string(&mut r, 1_048_576).ok()?;
            }
            (_, 9) => {
                // Array: element type + length, then skip by element width.
                let elem = read_u32(&mut r).ok()?;
                let len = read_u64(&mut r).ok()?;
                if len > 10_000_000 {
                    return None;
                }
                if leaf == "target_layers" {
                    meta.target_layers = Some(len);
                }
                // GGUF types: 0 u8, 1 i8, 2 u16, 3 i16, 4 u32, 5 i32,
                // 6 f32, 7 bool, 8 string, 10 u64, 11 i64, 12 f64.
                let width: u64 = match elem {
                    0 | 1 | 7 => 1,
                    2 => 2,
                    3 | 4 | 5 | 6 => 4,
                    10 | 11 | 12 => 8,
                    8 => {
                        // Array of strings: read each (capped).
                        for _ in 0..len {
                            read_gguf_string(&mut r, 1_048_576).ok()?;
                        }
                        continue;
                    }
                    _ => return None,
                };
                let skip = len.saturating_mul(width);
                std::io::Seek::seek(&mut r, std::io::SeekFrom::Current(skip as i64)).ok()?;
            }
            _ => {
                skip_value(&mut r, ty).ok()?;
            }
        }
    }
    // Tensor infos follow the KV block; read names best-effort to fingerprint
    // MTP heads. v1 layouts differ and are skipped.
    if version >= 2 {
        for _ in 0..n_tensors.min(100_000) {
            let Ok(name) = read_gguf_string(&mut r, 1024) else {
                break;
            };
            if name.ends_with(".nextn.eh_proj.weight") {
                meta.has_nextn = true;
            }
            let Ok(n_dims) = read_u32(&mut r) else {
                break;
            };
            if n_dims > 4 {
                break;
            }
            let mut ok = true;
            for _ in 0..n_dims {
                ok &= read_u64(&mut r).is_ok();
            }
            // Shape type + data offset.
            ok &= read_u32(&mut r).is_ok() && read_u64(&mut r).is_ok();
            if !ok {
                break;
            }
        }
    }
    Some(meta)
}

// ── Model info + scan ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub filename: String,
    pub path: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_b: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default)]
    pub is_vision: bool,
    #[serde(default)]
    pub is_reasoning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_repo: Option<String>,
}

/// Companion files are not loadable models. Names catch the common cases;
/// metadata catches the rest (EAGLE3/DFlash drafts, MTP heads).
fn is_auxiliary_file(name_lower: &str) -> bool {
    if name_lower.contains("mmproj")
        || name_lower.contains("dspark")
        || name_lower.contains("dflash")
        || name_lower.contains("imatrix")
    {
        return true;
    }
    let stem = name_lower.strip_suffix(".gguf").unwrap_or(name_lower);
    stem.split(['-', '_', ' ', '.']).next() == Some("mtp")
}

/// Metadata fingerprints of a speculative draft.
fn is_draft_metadata(meta: &ModelMetadata) -> bool {
    matches!(meta.architecture.as_deref(), Some("eagle3") | Some("dflash"))
        || meta.target_layers.is_some()
        || meta.has_nextn
}

/// Quant tag from the filename stem (`Q4_K_M`, `IQ2_XXS`, `F16`, …).
/// Matched against the whole stem so multi-part tags stay intact.
pub fn extract_quant(stem: &str) -> Option<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?i)[-._]((?:Q|IQ)\d[A-Z0-9_]*|MXFP\d+|F16|F32|BF16)$").unwrap()
    });
    re.captures(stem).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
}

pub fn format_params(params: u64) -> String {
    if params >= 1_000_000_000 {
        format!("{:.1}B", params as f64 / 1_000_000_000.0)
    } else {
        format!("{}M", params / 1_000_000)
    }
}

/// Parameter count from the filename (`...-7B-...`, `...-1.5B.gguf`).
fn extract_params_from_filename(filename: &str) -> Option<u32> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?i)[_\-.](\d+(?:\.\d+)?)b[_\-.]").unwrap()
    });
    let caps = re.captures(filename)?;
    caps.get(1)?.as_str().parse::<f64>().ok().map(|v| v as u32)
}

fn pretty_name(stem: &str) -> String {
    stem.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Hybrid SSM/linear-attention archs keep full KV in only some blocks:
/// (approximate attention-layer divisor, optional estimator note). Returns
/// None for standard transformer archs (every block has KV).
pub fn hybrid_kv_arch_divisor(arch: &str) -> Option<(f64, Option<String>)> {
    let note = format!(
        "Hybrid architecture ({arch}): only a fraction of blocks keep a KV cache — estimate is approximate."
    );
    match arch {
        // Full attention every block: no divisor needed, but non-shiftable.
        "gemma2" | "gemma3" | "gemma3n" | "gemma4" | "gemma4-assistant" | "gemma-embedding"
        | "llama4" | "exaone4" | "granite_swa" | "qwen2vl" | "qwen3vl" | "qwen3vlmoe" => None,
        // ~1 in 4 blocks is full attention; the rest are linear/SSM (no KV).
        "qwen3next" | "qwen35" | "qwen35moe" | "qwen4exp" | "granitehybrid" | "lfm2"
        | "lfm2moe" | "minimax-m2" | "kimi-linear" | "kimi-k3" | "glm-dsa" | "step35"
        | "nemotron-h" | "nemotron-h-moe" | "falcon-h1" | "jamba" => Some((4.0, Some(note))),
        // RWKV-family keeps token-shift state, not a growing KV cache.
        "mamba" | "mamba2" | "rwkv6" | "rwkv6qwen2" | "rwkv7" | "arwkv7" => {
            Some((f64::INFINITY, Some(note)))
        }
        _ => None,
    }
}

fn is_reasoning_model(meta: &ModelMetadata) -> bool {
    meta.chat_template.as_deref().map(|t| t.to_lowercase().contains("<think>")).unwrap_or(false)
}

/// Effort ids templates may accept; "none" counts when explicitly compared.
const EFFORT_IDS: &[&str] = &["minimal", "low", "medium", "high", "max", "xhigh", "none"];

fn effort_words_in(sentence: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in sentence.split(|c: char| !c.is_ascii_alphanumeric()) {
        let word = word.to_lowercase();
        if EFFORT_IDS.contains(&word.as_str()) && !out.iter().any(|w| w == &word) {
            out.push(word);
        }
    }
    out
}

/// Effort ids a chat template accepts (explicit sentence or
/// `reasoning_effort` comparisons; quoted literals only).
pub fn parse_reasoning_effort_levels(template: &str) -> Vec<String> {
    let lower = template.to_lowercase();
    // 1) Explicit sentence (ends at literal close, `}}`, or newline).
    for marker in ["supported types are", "supported values are", "supported efforts are"] {
        if let Some(idx) = lower.find(marker) {
            let rest = &lower[idx + marker.len()..];
            let cut = rest
                .find("')")
                .or_else(|| rest.find("'}}"))
                .or_else(|| rest.find('\n'))
                .unwrap_or(rest.len().min(160));
            let levels = effort_words_in(&rest[..cut]);
            if !levels.is_empty() {
                return levels;
            }
        }
    }
    // 2) Quoted literals on reasoning_effort lines.
    let mut out: Vec<String> = Vec::new();
    for line in template.lines() {
        if !line.to_lowercase().contains("reasoning_effort") {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let quote = chars[i];
            if quote != '\'' && quote != '"' {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            let mut token = String::new();
            while j < chars.len() && chars[j] != quote {
                token.push(chars[j]);
                j += 1;
            }
            let token = token.to_lowercase();
            if EFFORT_IDS.contains(&token.as_str()) && !out.iter().any(|w| w == &token) {
                out.push(token);
            }
            i = j + 1;
        }
    }
    out
}

/// True when the template uses effort knobs or an enable flag.
pub fn template_drives_reasoning(template: Option<&str>) -> bool {
    template.map_or(false, |t| {
        let lower = t.to_lowercase();
        lower.contains("reasoning_effort") || lower.contains("enable_thinking")
    })
}

/// All installed models across roots (recursive); companions excluded.
pub fn list_installed_models(roots: &[PathBuf]) -> Vec<ModelInfo> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cache = load_meta_cache();
    let mut dirty = false;
    for root in roots {
        scan_dir(root, &mut out, &mut seen, &mut cache, &mut dirty, 6);
    }
    // Drop entries for files no longer scanned so the cache cannot grow
    // stale paths (or test fixtures) forever.
    let before = cache.len();
    cache.retain(|k, _| seen.contains(k));
    if dirty || cache.len() != before {
        save_meta_cache(&cache);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Header-derived fields; filenames carry the rest. Validated by size+mtime.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct MetaCacheEntry {
    size_bytes: u64,
    mtime_secs: u64,
    /// Raw count from the header when present (older caches used this name too).
    #[serde(default, alias = "params_b")]
    parameter_count: Option<u64>,
    /// `general.size_label`, e.g. "7.2B".
    #[serde(default)]
    size_label: Option<String>,
    context_length: Option<u64>,
    architecture: Option<String>,
    is_reasoning: bool,
    /// The header fingerprints a draft (EAGLE3/DFlash/MTP); such files never
    /// list as models.
    #[serde(default)]
    is_draft: bool,
}

fn meta_cache_path() -> Option<PathBuf> {
    // Tests share one temp file instead of touching the real data dir.
    #[cfg(test)]
    {
        return Some(
            std::env::temp_dir()
                .join(format!("werk-test-cache-{}", std::process::id()))
                .join("gguf_cache2.json"),
        );
    }
    #[cfg(not(test))]
    {
        crate::config::AppConfig::config_path()
            .ok()?
            .parent()
            .map(|p| p.join("gguf_cache2.json"))
    }
}

fn load_meta_cache() -> std::collections::HashMap<String, MetaCacheEntry> {
    meta_cache_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn save_meta_cache(cache: &std::collections::HashMap<String, MetaCacheEntry>) {
    if let Some(path) = meta_cache_path() {
        if let Ok(content) = serde_json::to_string(&cache) {
            let _ = std::fs::write(path, content);
        }
    }
}

fn file_stat(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((meta.len(), mtime))
}

fn scan_dir(
    dir: &Path,
    out: &mut Vec<ModelInfo>,
    seen: &mut std::collections::HashSet<String>,
    cache: &mut std::collections::HashMap<String, MetaCacheEntry>,
    dirty: &mut bool,
    depth: usize,
) {
    if depth == 0 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            scan_dir(&path, out, seen, cache, dirty, depth - 1);
        } else if path.is_file() {
            let lower = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !lower.ends_with(".gguf") || is_auxiliary_file(&lower) {
                continue;
            }
            let key = path.to_string_lossy().to_lowercase();
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(info) = describe_cached(&path, &key, cache, dirty) {
                out.push(info);
            }
        }
    }
}

/// Model info via the header cache; uncached files pay one header read.
fn describe_cached(
    path: &Path,
    key: &str,
    cache: &mut std::collections::HashMap<String, MetaCacheEntry>,
    dirty: &mut bool,
) -> Option<ModelInfo> {
    let filename = path.file_name()?.to_string_lossy().to_string();
    let stem = filename.strip_suffix(".gguf").unwrap_or(&filename).to_string();
    let (size, mtime) = file_stat(path)?;
    let entry = match cache.get(key) {
        Some(e) if e.size_bytes == size && e.mtime_secs == mtime => e.clone(),
        _ => {
            let meta = read_model_metadata(path)?;
            let is_draft = is_draft_metadata(&meta);
            let is_reasoning = is_reasoning_model(&meta);
            let entry = MetaCacheEntry {
                size_bytes: size,
                mtime_secs: mtime,
                parameter_count: meta.parameter_count,
                size_label: meta.size_label,
                context_length: meta.context_length,
                architecture: meta.architecture,
                is_reasoning,
                is_draft,
            };
            cache.insert(key.to_string(), entry.clone());
            *dirty = true;
            entry
        }
    };
    if entry.is_draft {
        return None;
    }
    let params_b = entry
        .size_label
        .or_else(|| entry.parameter_count.map(format_params))
        .or_else(|| extract_params_from_filename(&filename).map(|p| format!("{p}B")));
    let mmproj_path = crate::server::find_mmproj_sibling(path)
        .map(|p| p.to_string_lossy().to_string());
    Some(ModelInfo {
        id: path.to_string_lossy().to_string(),
        name: pretty_name(&stem),
        filename: filename.clone(),
        path: path.to_string_lossy().to_string(),
        size_bytes: size,
        quant: extract_quant(&stem),
        params_b,
        context_length: entry.context_length,
        architecture: entry.architecture,
        is_vision: mmproj_path.is_some(),
        is_reasoning: entry.is_reasoning,
        mmproj_path,
        hf_repo: read_hf_sidecar(path),
    })
}

// ── HF sidecar ────────────────────────────────────────────────────────────

/// Download provenance: `{stem}.hf.json` next to the file.
pub fn hf_sidecar_path(model_path: &Path) -> Option<PathBuf> {
    let stem = model_path.file_stem()?.to_string_lossy().to_string();
    model_path.parent().map(|d| d.join(format!("{stem}.hf.json")))
}

pub fn write_hf_sidecar(model_path: &Path, repo_id: &str) {
    if let Some(sidecar) = hf_sidecar_path(model_path) {
        let text = format!("{{\"repo_id\": {}}}", serde_json::Value::String(repo_id.into()));
        let _ = std::fs::write(sidecar, text);
    }
}

pub fn read_hf_sidecar(model_path: &Path) -> Option<String> {
    let sidecar = hf_sidecar_path(model_path)?;
    let text = std::fs::read_to_string(sidecar).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("repo_id")?
        .as_str()
        .map(str::to_string)
}

// ── HuggingFace downloads ─────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct HfTreeEntry {
    pub path: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/// Recursive file tree for a repo (blobs only).
pub async fn hf_repo_files(
    client: &reqwest::Client,
    repo_id: &str,
) -> Result<Vec<HfTreeEntry>> {
    let url = format!("https://huggingface.co/api/models/{repo_id}/tree/main?recursive=true");
    let resp = client
        .get(&url)
        .header("User-Agent", "werk/0.1.0")
        .send()
        .await
        .with_context(|| format!("Listing {repo_id}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("Listing {repo_id} failed: {}", resp.status());
    }
    let entries: Vec<HfTreeEntry> = resp.json().await?;
    Ok(entries.into_iter().filter(|e| e.path.ends_with(".gguf")).collect())
}

fn encode_path(path: &str) -> String {
    path.replace(' ', "%20")
}

/// Download one repo file into `dest_dir`, resuming partials.
pub async fn download_hf_file(
    client: &reqwest::Client,
    repo_id: &str,
    filename: &str,
    dest_dir: &Path,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    download_hf_file_as(client, repo_id, filename, &dest_dir.join(filename), cancel, on_progress)
        .await
}

/// Partial-download path: `<dest>.part`, so a half file is never mistaken
/// for an installed model.
pub fn part_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| format!("{}.part", n.to_string_lossy()))
        .unwrap_or_else(|| "download.part".to_string());
    dest.with_file_name(name)
}

/// Download one repo file to an explicit destination (renames). The transfer
/// lands under `<dest>.part` and is renamed into place only when complete.
pub async fn download_hf_file_as(
    client: &reqwest::Client,
    repo_id: &str,
    filename: &str,
    dest: &Path,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    reject_traversal(filename)?;
    reject_traversal(&dest.to_string_lossy())?;
    let url = format!("https://huggingface.co/{repo_id}/resolve/main/{}", encode_path(filename));
    let part = part_path(dest);
    crate::download::download_to(client, &url, &part, cancel, |p| {
        on_progress(p.downloaded, p.total)
    })
    .await?;
    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    std::fs::rename(&part, dest).map_err(|e| anyhow::anyhow!("Cannot finish download: {e}"))?;
    Ok(dest.to_path_buf())
}

fn reject_traversal(path: &str) -> Result<()> {
    if path.contains("..") {
        anyhow::bail!("Unsafe path: {path}");
    }
    Ok(())
}

/// Basename of a repo path (`a/b.gguf` -> `b.gguf`).
pub fn repo_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Destination for a repo file: bare names nest under `owner/model`,
/// repo paths keep their layout; renames land beside the main file.
pub fn download_dest(
    dest_dir: &Path,
    repo_id: &str,
    filename: &str,
    save_as: Option<&str>,
) -> PathBuf {
    let mut folder = dest_dir.to_path_buf();
    if !filename.contains('/') {
        let mut parts = repo_id.split('/');
        if let (Some(owner), Some(model)) = (parts.next(), parts.next()) {
            if !owner.is_empty() && !model.is_empty() && !repo_id.contains("..") {
                folder = folder.join(owner).join(model);
            }
        }
    }
    match save_as {
        Some(name) => folder.join(name),
        None => folder.join(filename),
    }
}

// ── HuggingFace search ────────────────────────────────────────────────────

/// Wire search hit (counts fit u32 with room to spare).
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct HfModel {
    pub repo_id: String,
    pub name: String,
    pub author: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub downloads: u32,
    pub likes: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct HfSearchHit {
    #[serde(default)]
    id: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    downloads: u32,
    #[serde(default)]
    likes: u32,
}

pub async fn search_hf(
    client: &reqwest::Client,
    query: &str,
    sort: &str,
) -> Result<Vec<HfModel>> {
    let sort = match sort {
        "likes" | "lastModified" => sort,
        _ => "downloads",
    };
    let mut url = reqwest::Url::parse("https://huggingface.co/api/models")
        .context("Bad search URL")?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("search", query)
            .append_pair("sort", sort)
            .append_pair("direction", "-1")
            .append_pair("limit", "25")
            .append_pair("filter", "gguf");
    }
    let resp = client
        .get(url)
        .header("User-Agent", "werk/0.1.0")
        .send()
        .await
        .context("Searching HuggingFace")?;
    if !resp.status().is_success() {
        anyhow::bail!("Search failed: {}", resp.status());
    }
    let hits: Vec<HfSearchHit> = resp.json().await?;
    Ok(hits
        .into_iter()
        .filter(|h| !h.id.is_empty())
        .map(|h| {
            let stem = h.id.split('/').nth(1).unwrap_or(&h.id).to_string();
            // The search API carries no author field; the owner is the id prefix.
            let author = if h.author.is_empty() {
                h.id.split('/').next().unwrap_or("").to_string()
            } else {
                h.author
            };
            HfModel {
                repo_id: h.id,
                name: pretty_name(&stem),
                author,
                tags: h.tags,
                downloads: h.downloads,
                likes: h.likes,
            }
        })
        .collect())
}

// ── Delete ────────────────────────────────────────────────────────────────

/// Split-GGUF suffix (`model-00001-of-00005.gguf`); returns (base, index, total).
fn split_suffix(filename: &str) -> Option<(String, u32, u32)> {
    let stem = filename.strip_suffix(".gguf")?;
    let (rest, total) = stem.rsplit_once("-of-")?;
    let total: u32 = total.parse().ok()?;
    let (base, index) = rest.rsplit_once('-')?;
    let index: u32 = index.parse().ok()?;
    if total < 2 || index == 0 || index > total {
        return None;
    }
    Some((base.to_string(), index, total))
}

/// Delete a model (all split parts) plus orphaned sidecars when the last
/// model in its folder goes; prunes emptied folders up to the roots.
pub fn delete_model(path: &Path, model_roots: &[PathBuf]) -> Result<()> {
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let parent = path.parent().map(|p| p.to_path_buf());

    if let Some((base, _, total)) = split_suffix(filename) {
        if let Some(dir) = &parent {
            for i in 1..=total {
                let part = dir.join(format!("{base}-{i:05}-of-{total:05}.gguf"));
                if part.exists() {
                    std::fs::remove_file(&part)?;
                }
            }
        }
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }

    if let Some(dir) = &parent {
        let is_model = |name: &str| {
            name.ends_with(".gguf")
                && !name.contains("mmproj")
                && !name.contains("dspark")
                && !name.contains("imatrix")
        };
        let remaining = std::fs::read_dir(dir)
            .map(|entries| {
                entries.flatten().any(|e| {
                    is_model(&e.file_name().to_string_lossy().to_lowercase())
                })
            })
            .unwrap_or(false);
        if !remaining {
            for entry in std::fs::read_dir(dir)?.flatten() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let lower = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                let orphan = lower.ends_with(".jinja")
                    || lower.ends_with(".hf.json")
                    || (lower.ends_with(".gguf")
                        && (lower.contains("mmproj")
                            || lower.contains("dspark")
                            || lower.contains("imatrix")));
                if orphan {
                    std::fs::remove_file(&p)?;
                }
            }
        }
    }

    // Prune emptied folders up to (not including) the model roots.
    if let Some(mut dir) = parent {
        loop {
            let at_root = model_roots.iter().any(|r| {
                dir.to_string_lossy().to_lowercase().trim_end_matches(['\\', '/'])
                    == r.to_string_lossy().to_lowercase().trim_end_matches(['\\', '/'])
            });
            if at_root {
                break;
            }
            let is_empty = std::fs::read_dir(&dir).map(|mut e| e.next().is_none()).unwrap_or(false);
            if !is_empty {
                break;
            }
            std::fs::remove_dir(&dir)?;
            match dir.parent() {
                Some(parent) => dir = parent.to_path_buf(),
                None => break,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_gguf(path: &Path, kvs: &[(&str, u8, Vec<u8>)]) {
        // Minimal GGUF: magic, version, tensor count, KV count, then KVs.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"GGUF");
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes());
        buf.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
        for (key, ty, val) in kvs {
            let kb = key.as_bytes();
            buf.extend_from_slice(&(kb.len() as u64).to_le_bytes());
            buf.extend_from_slice(kb);
            buf.extend_from_slice(&(*ty as u32).to_le_bytes());
            buf.extend_from_slice(val);
        }
        std::fs::write(path, buf).unwrap();
    }

    fn u32_val(v: u32) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    fn str_val(s: &str) -> Vec<u8> {
        let mut v = (s.len() as u64).to_le_bytes().to_vec();
        v.extend_from_slice(s.as_bytes());
        v
    }

    /// Like `write_gguf`, but with tensor infos after the KVs.
    fn write_gguf_tensors(path: &Path, kvs: &[(&str, u8, Vec<u8>)], tensors: &[&str]) {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"GGUF");
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&(tensors.len() as u64).to_le_bytes());
        buf.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
        for (key, ty, val) in kvs {
            let kb = key.as_bytes();
            buf.extend_from_slice(&(kb.len() as u64).to_le_bytes());
            buf.extend_from_slice(kb);
            buf.extend_from_slice(&(*ty as u32).to_le_bytes());
            buf.extend_from_slice(val);
        }
        for name in tensors {
            let nb = name.as_bytes();
            buf.extend_from_slice(&(nb.len() as u64).to_le_bytes());
            buf.extend_from_slice(nb);
            buf.extend_from_slice(&1u32.to_le_bytes()); // n_dims
            buf.extend_from_slice(&8u64.to_le_bytes()); // dims[0]
            buf.extend_from_slice(&0u32.to_le_bytes()); // type
            buf.extend_from_slice(&0u64.to_le_bytes()); // offset
        }
        std::fs::write(path, buf).unwrap();
    }

    #[test]
    fn draft_fingerprints_from_metadata() {
        let dir = std::env::temp_dir().join(format!("werk-gguf-draft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // EAGLE3: arch string plus a target_layers array.
        let eagle = dir.join("eagle.gguf");
        let mut layers = 4u32.to_le_bytes().to_vec();
        layers.extend_from_slice(&3u64.to_le_bytes());
        for v in [2u32, 15, 27] {
            layers.extend_from_slice(&v.to_le_bytes());
        }
        write_gguf(
            &eagle,
            &[
                ("general.architecture", 8, str_val("eagle3")),
                ("eagle3.target_layers", 9, layers),
            ],
        );
        let meta = read_model_metadata(&eagle).expect("parses");
        assert_eq!(meta.architecture.as_deref(), Some("eagle3"));
        assert_eq!(meta.target_layers, Some(3));

        // MTP head: a nextn tensor with no telling filename metadata.
        let mtp = dir.join("mystery.gguf");
        write_gguf_tensors(
            &mtp,
            &[("general.architecture", 8, str_val("llama"))],
            &["blk.0.attn_q.weight", "blk.31.nextn.eh_proj.weight"],
        );
        let meta = read_model_metadata(&mtp).expect("parses");
        assert!(meta.has_nextn);
        assert_eq!(meta.target_layers, None);
        assert_eq!(meta.architecture.as_deref(), Some("llama"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_gguf_metadata_by_suffix() {
        let dir = std::env::temp_dir().join(format!("werk-gguf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.gguf");
        write_gguf(
            &path,
            &[
                ("general.architecture", 8, str_val("llama")),
                ("llama.context_length", 4, u32_val(131072)),
                ("llama.block_count", 4, u32_val(80)),
                ("general.size_label", 8, str_val("7.2B")),
                ("tokenizer.chat_template", 8, str_val("{% if True %}<think>{% endif %}")),
            ],
        );
        let meta = read_model_metadata(&path).expect("parses");
        assert_eq!(meta.architecture.as_deref(), Some("llama"));
        assert_eq!(meta.context_length, Some(131072));
        assert_eq!(meta.block_count, Some(80));
        assert_eq!(meta.size_label.as_deref(), Some("7.2B"));
        assert!(meta.chat_template.unwrap().contains("<think>"));
        assert!(read_model_metadata(&dir.join("missing.gguf")).is_none());
        std::fs::write(dir.join("junk.gguf"), b"not a gguf file at all!!!!").unwrap();
        assert!(read_model_metadata(&dir.join("junk.gguf")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_list_hides_metadata_drafts() {
        let dir = std::env::temp_dir().join(format!("werk-list-draft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_gguf(
            &dir.join("real.gguf"),
            &[("general.architecture", 8, str_val("llama"))],
        );
        // EAGLE3 drafts rarely say so in the filename; the arch does.
        write_gguf(
            &dir.join("mystery.gguf"),
            &[("general.architecture", 8, str_val("eagle3"))],
        );
        // Target layers alone fingerprint a draft too.
        let mut layers = 4u32.to_le_bytes().to_vec();
        layers.extend_from_slice(&3u64.to_le_bytes());
        for v in [2u32, 15, 27] {
            layers.extend_from_slice(&v.to_le_bytes());
        }
        write_gguf(
            &dir.join("layers.gguf"),
            &[("llama.target_layers", 9, layers)],
        );
        let models = list_installed_models(&[dir.clone()]);
        let names: Vec<&str> = models.iter().map(|m| m.filename.as_str()).collect();
        assert_eq!(names, vec!["real.gguf"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quant_and_params_format() {
        assert_eq!(extract_quant("Model-Q4_K_M"), Some("Q4_K_M".to_string()));
        assert_eq!(extract_quant("Model-IQ2_XXS"), Some("IQ2_XXS".to_string()));
        assert_eq!(extract_quant("Model-F16"), Some("F16".to_string()));
        assert_eq!(extract_quant("plain-model"), None);
        assert_eq!(format_params(7_200_000_000), "7.2B");
        assert_eq!(format_params(800_000_000), "800M");
    }

    #[test]
    fn params_from_filename_patterns() {
        assert_eq!(extract_params_from_filename("Llama-3.1-8B-Q4_K_M.gguf"), Some(8));
        assert_eq!(extract_params_from_filename("model-70B-Q5_K.gguf"), Some(70));
        assert_eq!(extract_params_from_filename("qwen2.5-1.5b-instruct-Q4_K_M.gguf"), Some(1));
        assert_eq!(extract_params_from_filename("model.gguf"), None);
    }

    #[test]
    fn reasoning_levels_from_supported_sentence() {
        let template = "{{- raise_exception('Unexpected reasoning effort ' ~ reasoning_effort ~ '. Supported types are xhigh (default), medium, and low.') }}";
        assert_eq!(
            parse_reasoning_effort_levels(template),
            vec!["xhigh".to_string(), "medium".to_string(), "low".to_string()]
        );
    }

    #[test]
    fn reasoning_levels_from_comparisons() {
        let template = "{%- if reasoning_effort == 'low' %}\n<think_low>\n{%- elif reasoning_effort == \"medium\" %}\n<think>\n{%- endif -%}";
        assert_eq!(
            parse_reasoning_effort_levels(template),
            vec!["low".to_string(), "medium".to_string()]
        );
    }

    #[test]
    fn reasoning_levels_absent_without_markers() {
        assert!(parse_reasoning_effort_levels("plain template").is_empty());
        assert!(template_drives_reasoning(Some("{% if enable_thinking %}")));
        assert!(!template_drives_reasoning(None));
    }

    #[test]
    fn real_world_types_parse() {
        // bool, f32, u64, and string arrays use their spec numbers;
        // anything else must not silently misalign the stream.
        let dir = std::env::temp_dir().join(format!("werk-types-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.gguf");
        let mut tags = (8u32.to_le_bytes()).to_vec();
        tags.extend_from_slice(&(1u64.to_le_bytes()));
        tags.extend_from_slice(&str_val("llama"));
        write_gguf(
            &path,
            &[
                ("general.architecture", 8, str_val("llama")),
                ("tokenizer.ggml.add_bos_token", 7, vec![1]),
                ("general.parameter_count", 10, 7_200_000_000u64.to_le_bytes().to_vec()),
                ("general.tags", 9, tags),
                ("llama.context_length", 4, 131072u32.to_le_bytes().to_vec()),
            ],
        );
        let meta = read_model_metadata(&path).expect("parses mixed types");
        assert_eq!(meta.architecture.as_deref(), Some("llama"));
        assert_eq!(meta.parameter_count, Some(7_200_000_000));
        assert_eq!(meta.context_length, Some(131072));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_lists_models_skips_companions() {
        let dir = std::env::temp_dir().join(format!("werk-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_gguf(&dir.join("a-Q4_K_M.gguf"), &[("general.architecture", 8, str_val("x"))]);
        std::fs::write(dir.join("mmproj.gguf"), b"junk-but-named-right").unwrap();
        std::fs::write(dir.join("notes.txt"), b"nope").unwrap();
        let models = list_installed_models(&[dir.clone()]);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(models[0].name, "a Q4 K M");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_finds_nested_models() {
        let dir = std::env::temp_dir().join(format!("werk-nest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nested = dir.join("owner").join("model");
        std::fs::create_dir_all(&nested).unwrap();
        write_gguf(&nested.join("m.gguf"), &[("general.architecture", 8, str_val("x"))]);
        let models = list_installed_models(&[dir.clone()]);
        assert_eq!(models.len(), 1);
        assert!(models[0].path.contains("owner"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_hits_and_invalidates() {
        let dir = std::env::temp_dir().join(format!("werk-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("m-8B.gguf");
        write_gguf(&file, &[("general.architecture", 8, str_val("llama"))]);
        let first = list_installed_models(&[dir.clone()]);
        assert_eq!(first.len(), 1);
        // Same bytes: the cached entry serves the second scan.
        let second = list_installed_models(&[dir.clone()]);
        assert_eq!(second, first);
        // Changed size: the entry refreshes instead of going stale.
        std::fs::write(&file, std::fs::read(&file).unwrap().repeat(2)).unwrap();
        let third = list_installed_models(&[dir.clone()]);
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].size_bytes, first[0].size_bytes * 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_dest_nests_bare_names() {
        let root = PathBuf::from("/models");
        assert_eq!(
            download_dest(&root, "owner/repo", "m.gguf", None),
            root.join("owner").join("repo").join("m.gguf")
        );
        assert_eq!(
            download_dest(&root, "owner/repo", "sub/m.gguf", None),
            root.join("sub/m.gguf")
        );
        assert_eq!(
            download_dest(&root, "owner/repo", "m.gguf", Some("m-mmproj.gguf")),
            root.join("owner").join("repo").join("m-mmproj.gguf")
        );
        assert_eq!(
            download_dest(&root, "weird", "m.gguf", None),
            root.join("m.gguf")
        );
    }

    #[test]
    fn sidecar_round_trips_repo() {
        let dir = std::env::temp_dir().join(format!("werk-hf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("m.gguf");
        std::fs::write(&model, b"x").unwrap();
        assert_eq!(read_hf_sidecar(&model), None);
        write_hf_sidecar(&model, "owner/repo");
        assert_eq!(read_hf_sidecar(&model).as_deref(), Some("owner/repo"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn part_path_suffixes_the_file_name() {
        assert_eq!(
            part_path(Path::new("/a/model.gguf")),
            PathBuf::from("/a/model.gguf.part")
        );
        assert_eq!(part_path(Path::new("/a/noext")), PathBuf::from("/a/noext.part"));
    }

    #[test]
    fn delete_removes_parts_companions_and_empty_dirs() {
        let root = std::env::temp_dir().join(format!("werk-del-{}", std::process::id()));
        let repo = root.join("owner").join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("m-00001-of-00002.gguf"), b"a").unwrap();
        std::fs::write(repo.join("m-00002-of-00002.gguf"), b"b").unwrap();
        std::fs::write(repo.join("m.jinja"), b"j").unwrap();
        std::fs::write(repo.join("m.hf.json"), b"{}".as_slice()).unwrap();
        delete_model(&repo.join("m-00001-of-00002.gguf"), &[root.clone()]).unwrap();
        assert!(!repo.join("m-00001-of-00002.gguf").exists());
        assert!(!repo.join("m-00002-of-00002.gguf").exists(), "all parts go");
        assert!(!repo.join("m.jinja").exists(), "orphaned template goes");
        assert!(!repo.join("m.hf.json").exists(), "orphaned sidecar goes");
        assert!(!repo.exists(), "emptied folder pruned");
        assert!(root.exists(), "models root must remain");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_keeps_companions_while_models_remain() {
        let root = std::env::temp_dir().join(format!("werk-keep-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.gguf"), b"a").unwrap();
        std::fs::write(root.join("b.gguf"), b"b").unwrap();
        std::fs::write(root.join("a.jinja"), b"j").unwrap();
        delete_model(&root.join("a.gguf"), &[root.clone()]).unwrap();
        assert!(!root.join("a.gguf").exists());
        assert!(root.join("b.gguf").exists());
        assert!(root.join("a.jinja").exists(), "template stays while models remain");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn split_suffix_parses() {
        assert_eq!(
            split_suffix("m-00001-of-00002.gguf"),
            Some(("m".to_string(), 1, 2))
        );
        assert_eq!(split_suffix("plain.gguf"), None);
        assert_eq!(split_suffix("m-00000-of-00002.gguf"), None);
    }
}
