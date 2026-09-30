//! MCP servers: Cursor-compatible `mcp.json` plus an effective copy for
//! launch (disabled entries filtered, Windows script shims wrapped).

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServer {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct McpServerEntry {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    /// App-side toggle; persisted separately so mcp.json stays portable.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers")]
    pub servers: BTreeMap<String, McpServer>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct McpInfo {
    pub path: String,
    pub servers: Vec<McpServerEntry>,
}

pub fn mcp_config_path() -> Result<PathBuf> {
    let config_path = crate::config::AppConfig::config_path()?;
    let base = config_path.parent().context("Config path has no parent")?;
    Ok(base.join("mcp.json"))
}

fn effective_path(base: &Path) -> Result<PathBuf> {
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mcp");
    Ok(base.with_file_name(format!("{stem}_effective.json")))
}

fn remove_stale_effective(base: &Path) {
    if let Ok(path) = effective_path(base) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn load_at(path: &Path) -> Result<McpConfig> {
    if !path.exists() {
        return Ok(McpConfig::default());
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Invalid MCP config {}: {e}", path.display()))
}

pub fn save_at(entries: &[McpServerEntry], path: &Path) -> Result<()> {
    let mut servers = BTreeMap::new();
    for e in entries {
        let name = e.name.trim();
        let command = e.command.trim();
        // Entries without a command are skipped by llama.cpp; don't persist them.
        if name.is_empty() || command.is_empty() {
            continue;
        }
        servers.insert(
            name.to_string(),
            McpServer {
                command: command.to_string(),
                args: e.args.iter().filter(|a| !a.is_empty()).cloned().collect(),
                env: e.env.clone(),
                cwd: e
                    .cwd
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .map(str::to_string),
                timeout_ms: e.timeout_ms,
            },
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&McpConfig { servers })?)?;
    Ok(())
}

pub fn entries_from_config(cfg: &McpConfig) -> Vec<McpServerEntry> {
    cfg.servers
        .iter()
        .map(|(name, s)| McpServerEntry {
            name: name.clone(),
            command: s.command.clone(),
            args: s.args.clone(),
            env: s.env.clone(),
            cwd: s.cwd.clone(),
            timeout_ms: s.timeout_ms,
            enabled: true,
        })
        .collect()
}

pub fn filter_disabled(cfg: &McpConfig, disabled: &[String]) -> McpConfig {
    let mut out = cfg.clone();
    out.servers.retain(|name, _| !disabled.iter().any(|d| d == name));
    out
}

/// Keyless defaults seeded on first run.
pub fn default_entries() -> Vec<McpServerEntry> {
    vec![McpServerEntry {
        name: "ddg-search".to_string(),
        command: "uvx".to_string(),
        args: vec![
            "--with".to_string(),
            "duckduckgo-mcp-server[browser]".to_string(),
            "duckduckgo-mcp-server".to_string(),
        ],
        env: HashMap::new(),
        cwd: None,
        timeout_ms: None,
        enabled: true,
    }]
}

/// Seed defaults only when mcp.json is missing (respects deletions).
pub fn ensure_defaults(path: &Path) -> Result<()> {
    if !path.exists() {
        save_at(&default_entries(), path)?;
    }
    Ok(())
}

/// Enabled servers as harness specs (shim-wrapped so werk can spawn them).
pub fn agent_specs(disabled: &[String]) -> Result<Vec<harness::mcp::McpServerSpec>> {
    let path = mcp_config_path()?;
    let cfg = apply_shim_wrap(&filter_disabled(&load_at(&path)?, disabled));
    Ok(cfg
        .servers
        .into_iter()
        .filter(|(_, s)| !s.command.trim().is_empty())
        .map(|(name, s)| harness::mcp::McpServerSpec {
            name,
            command: s.command,
            args: s.args,
            env: s.env,
            cwd: s.cwd,
            timeout_ms: s.timeout_ms,
        })
        .collect())
}

fn spec_fingerprint(specs: &[harness::mcp::McpServerSpec], approval: bool) -> u64 {    let mut h = std::collections::hash_map::DefaultHasher::new();
    approval.hash(&mut h);
    for spec in specs {
        spec.name.hash(&mut h);
        spec.command.hash(&mut h);
        spec.args.hash(&mut h);
        spec.cwd.hash(&mut h);
        spec.timeout_ms.hash(&mut h);
        let mut env: Vec<_> = spec.env.iter().collect();
        env.sort();
        for (k, v) in env {
            k.hash(&mut h);
            v.hash(&mut h);
        }
    }
    h.finish()
}

/// Disk cache of tool schemas, so runs never spawn servers just to list tools.
fn tool_cache_path() -> Result<PathBuf> {
    Ok(mcp_config_path()?.with_file_name("mcp_tools_cache.json"))
}

#[derive(Serialize, Deserialize)]
struct ToolCacheFile {
    #[serde(default)]
    servers: Vec<harness::mcp::CachedServer>,
}

pub fn load_tool_cache() -> Vec<harness::mcp::CachedServer> {
    let Ok(path) = tool_cache_path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    serde_json::from_str::<ToolCacheFile>(&text)
        .map(|f| f.servers)
        .unwrap_or_default()
}

pub fn save_tool_cache(servers: &[harness::mcp::CachedServer]) {
    let Ok(path) = tool_cache_path() else { return };
    let Ok(text) = serde_json::to_string_pretty(&ToolCacheFile { servers: servers.to_vec() }) else {
        return;
    };
    let _ = std::fs::write(path, text);
}

/// Persistent, lazily started MCP servers shared across agent runs; rebuilt
/// when `mcp.json` changes and killed on app exit.
#[derive(Default)]
pub struct McpAgent {
    current: Mutex<Option<(u64, Arc<harness::mcp::McpManager>)>>,
}

impl McpAgent {
    pub fn manager(
        &self,
        specs: &[harness::mcp::McpServerSpec],
        approval: bool,
    ) -> Arc<harness::mcp::McpManager> {
        let fingerprint = spec_fingerprint(specs, approval);
        let mut guard = self.current.lock().unwrap();
        if let Some((fp, manager)) = guard.as_ref() {
            if *fp == fingerprint {
                return manager.clone();
            }
        }
        if let Some((_, old)) = guard.take() {
            save_tool_cache(&old.cache_snapshot());
            old.shutdown();
        }
        let manager = Arc::new(harness::mcp::McpManager::new(
            specs.to_vec(),
            approval,
            load_tool_cache(),
        ));
        *guard = Some((fingerprint, manager.clone()));
        manager
    }

    /// Persist discovered schemas now (after a discovery pass).
    pub fn persist_cache(&self, manager: &harness::mcp::McpManager) {
        save_tool_cache(&manager.cache_snapshot());
    }

    /// Drop the manager (killing children) — on exit and config changes.
    pub fn shutdown(&self) {
        if let Some((_, manager)) = self.current.lock().unwrap().take() {
            save_tool_cache(&manager.cache_snapshot());
            manager.shutdown();
        }
    }
}

// ── Windows script shims ────────────────────────────────────────────────────
// llama.cpp spawns MCP children directly, which cannot launch `.cmd`/`.bat`
// shims (npx, uvx); wrap those as `cmd /c`, keep mcp.json portable.

fn is_script_shim(resolved: &Path) -> bool {
    resolved
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
}

fn search_path(command: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if command.contains('/') || command.contains('\\') {
            let direct = dir.join(command);
            if direct.is_file() {
                return Some(direct);
            }
            continue;
        }
        for ext in ["", ".exe", ".cmd", ".bat", ".com"] {
            let candidate = dir.join(format!("{command}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Rewrite script-shim commands to `cmd /c <command> <args>`.
pub fn apply_shim_wrap(cfg: &McpConfig) -> McpConfig {
    let mut out = cfg.clone();
    for server in out.servers.values_mut() {
        if server.command.trim().is_empty() {
            continue;
        }
        let already_wrapped = server.command.eq_ignore_ascii_case("cmd")
            && server.args.first().map(|a| a == "/c").unwrap_or(false);
        if already_wrapped {
            continue;
        }
        let needs_wrap = match search_path(&server.command) {
            Some(resolved) => is_script_shim(&resolved),
            // Unresolvable here may still resolve in the server's
            // environment — but a bare script-ish name almost certainly
            // needs the wrapper, so wrap conservatively.
            None => {
                let lower = server.command.to_lowercase();
                lower.ends_with(".cmd") || lower.ends_with(".bat")
            }
        };
        if needs_wrap {
            let mut args = vec!["/c".to_string(), server.command.clone()];
            args.extend(server.args.iter().cloned());
            server.command = "cmd".to_string();
            server.args = args;
        }
    }
    out
}

/// Effective launch config: disabled filtered, shims wrapped. Returns None
/// when nothing is configured. Reuses mcp.json when it already matches.
pub fn runtime_mcp_config_path(disabled: &[String]) -> Result<Option<PathBuf>> {
    let base = mcp_config_path()?;
    let raw = load_at(&base)?;
    let cfg = apply_shim_wrap(&filter_disabled(&raw, disabled));
    if cfg.servers.is_empty() {
        remove_stale_effective(&base);
        return Ok(None);
    }
    if serde_json::to_string(&cfg)? == serde_json::to_string(&raw)? {
        remove_stale_effective(&base);
        return Ok(Some(base));
    }
    let path = effective_path(&base)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&cfg)?)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("werk-mcp-{}-{}.json", std::process::id(), label))
    }

    fn sample_entries() -> Vec<McpServerEntry> {
        vec![McpServerEntry {
            name: "fetch".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "x".to_string()],
            env: HashMap::new(),
            cwd: None,
            timeout_ms: Some(15000),
            enabled: true,
        }]
    }

    #[test]
    fn save_load_round_trip() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        save_at(&sample_entries(), &path).unwrap();
        let cfg = load_at(&path).unwrap();
        let entries = entries_from_config(&cfg);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timeout_ms, Some(15000));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn commandless_entries_dropped() {
        let path = temp_path("drop");
        let _ = std::fs::remove_file(&path);
        let mut entries = sample_entries();
        entries.push(McpServerEntry {
            name: "broken".to_string(),
            command: "   ".to_string(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            timeout_ms: None,
            enabled: true,
        });
        save_at(&entries, &path).unwrap();
        let cfg = load_at(&path).unwrap();
        assert_eq!(cfg.servers.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn defaults_seed_ddg_only_when_missing() {
        let path = temp_path("defaults");
        let _ = std::fs::remove_file(&path);
        ensure_defaults(&path).unwrap();
        let cfg = load_at(&path).unwrap();
        assert!(cfg.servers.contains_key("ddg-search"));
        // Existing files are respected: deletions stick.
        save_at(&[], &path).unwrap();
        ensure_defaults(&path).unwrap();
        assert!(load_at(&path).unwrap().servers.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disabled_filtered() {
        let mut cfg = McpConfig::default();
        cfg.servers.insert("a".to_string(), McpServer { command: "x".to_string(), ..Default::default() });
        cfg.servers.insert("b".to_string(), McpServer { command: "y".to_string(), ..Default::default() });
        let kept = filter_disabled(&cfg, &["a".to_string()]);
        assert!(!kept.servers.contains_key("a"));
        assert!(kept.servers.contains_key("b"));
    }

    #[test]
    fn explicit_script_names_wrapped() {
        let mut cfg = McpConfig::default();
        cfg.servers.insert(
            "s".to_string(),
            McpServer { command: "tool.cmd".to_string(), args: vec!["a".to_string()], ..Default::default() },
        );
        let wrapped = apply_shim_wrap(&cfg);
        let s = &wrapped.servers["s"];
        assert_eq!(s.command, "cmd");
        assert_eq!(s.args, vec!["/c", "tool.cmd", "a"]);
    }

    #[test]
    fn plain_executables_untouched() {
        let mut cfg = McpConfig::default();
        cfg.servers.insert(
            "s".to_string(),
            McpServer { command: "C:/tools/srv.exe".to_string(), args: vec![], ..Default::default() },
        );
        let kept = apply_shim_wrap(&cfg);
        assert_eq!(kept.servers["s"].command, "C:/tools/srv.exe");
    }
}
