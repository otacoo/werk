//! Server presets: named tuning files plus a per-model memory.
//! Files hold tuning only; model, mmproj, and working dir are per-session,
//! stripped on save and kept by the caller on load.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::server::ServerConfig;

/// User defaults, applied when no model preset matches.
pub const DEFAULT_PRESET: &str = "__default__";

/// `<data>/werk/presets`, created on first save.
pub fn presets_dir() -> Result<PathBuf> {
    let config_path = crate::config::AppConfig::config_path()?;
    let base = config_path.parent().context("Config path has no parent")?;
    Ok(base.join("presets"))
}

fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("Preset name must be 1-64 characters");
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ')
    {
        anyhow::bail!("Preset name allows letters, numbers, space, - and _");
    }
    Ok(())
}

fn preset_path(dir: &Path, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    Ok(dir.join(format!("{name}.json")))
}

/// Sorted preset names; a missing dir means none.
pub fn list_presets(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return names;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if check_name(stem).is_ok() {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    names
}

pub fn load_preset(dir: &Path, name: &str) -> Result<ServerConfig> {
    let path = preset_path(dir, name)?;
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("Preset '{name}' not found"))?;
    serde_json::from_str(&content).with_context(|| format!("Preset '{name}' is corrupt"))
}

pub fn save_preset(dir: &Path, name: &str, mut config: ServerConfig) -> Result<()> {
    let path = preset_path(dir, name)?;
    config.model_path.clear();
    config.mmproj_path = None;
    config.working_dir = None;
    std::fs::create_dir_all(dir)?;
    let content = serde_json::to_string_pretty(&config)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn delete_preset(dir: &Path, name: &str) -> Result<()> {
    let path = preset_path(dir, name)?;
    if !path.is_file() {
        anyhow::bail!("Preset '{name}' not found");
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("werk-preset-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sample() -> ServerConfig {
        ServerConfig {
            model_path: "/m/qwen.gguf".into(),
            mmproj_path: Some("/m/mmproj.gguf".into()),
            n_ctx: 32768,
            working_dir: Some("/work".into()),
            ..Default::default()
        }
    }

    #[test]
    fn save_strips_session_fields_and_load_round_trips() {
        let dir = scratch("roundtrip");
        save_preset(&dir, "fast", sample()).unwrap();
        let loaded = load_preset(&dir, "fast").unwrap();
        assert_eq!(loaded.n_ctx, 32768);
        assert!(loaded.model_path.is_empty());
        assert!(loaded.mmproj_path.is_none());
        assert!(loaded.working_dir.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_sorts_and_skips_foreign_files() {
        let dir = scratch("list");
        std::fs::create_dir_all(&dir).unwrap();
        save_preset(&dir, "zeta", ServerConfig::default()).unwrap();
        save_preset(&dir, "alpha", ServerConfig::default()).unwrap();
        std::fs::write(dir.join("notes.txt"), "hi").unwrap();
        std::fs::write(dir.join("evil..json"), "{}").unwrap();
        assert_eq!(list_presets(&dir), vec!["alpha", "zeta"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_names_rejected() {
        let dir = scratch("names");
        assert!(save_preset(&dir, "", ServerConfig::default()).is_err());
        assert!(save_preset(&dir, "../x", ServerConfig::default()).is_err());
        assert!(save_preset(&dir, "a/b", ServerConfig::default()).is_err());
        assert!(load_preset(&dir, "..").is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn missing_and_corrupt_fail_loudly() {
        let dir = scratch("missing");
        assert!(load_preset(&dir, "ghost").is_err());
        assert!(delete_preset(&dir, "ghost").is_err());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.json"), "{nope").unwrap();
        assert!(load_preset(&dir, "bad").is_err());
        delete_preset(&dir, "bad").unwrap();
        assert!(list_presets(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
