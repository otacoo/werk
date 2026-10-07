//! Portable assistant bundles: identity, memory, skills, plugins, and an
//! optional transcript in one `.werk` file, so a setup can be shared or
//! moved between machines.
//!
//! Imports are guarded: skills and plugins never overwrite an existing entry
//! (they get a suffix), imported plugins always ask for approval, and
//! system-control tools stay off until the user enables them by hand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use harness::client::ChatMessage;

pub const BUNDLE_FORMAT: &str = "werk-bundle";
pub const BUNDLE_VERSION: u32 = 1;

/// Import caps: a bundle is untrusted input, so bound what it can write.
const MEMORY_LINES_CAP: usize = 500;
const SKILLS_CAP: usize = 50;
const PLUGINS_CAP: usize = 20;
const PLUGIN_FILE_CAP: usize = 512 * 1024;
const PLUGIN_TOTAL_CAP: usize = 2 * 1024 * 1024;
const TRANSCRIPT_CAP: usize = 2_000;

/// Which parts of a bundle to write on export, and which to apply on import.
/// Export always carries the identity; import always ignores the tool toggles
/// (they are shown, never enabled).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct BundleInclude {
    /// Import only: replace the assistant identity and generation settings.
    pub identity: bool,
    pub memory: bool,
    pub skills: bool,
    pub plugins: bool,
    pub transcript: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleGeneration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleSkill {
    pub name: String,
    pub description: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundlePlugin {
    pub name: String,
    /// Raw `plugin.json`; validated on import.
    pub manifest: serde_json::Value,
    /// Co-located script files, keyed by plain file name.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub format: String,
    pub version: u32,
    pub kind: String,
    pub created: u32,
    #[serde(default)]
    pub app: String,
    pub name: String,
    #[serde(default)]
    pub persona: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub generation: BundleGeneration,
    #[serde(default)]
    pub memory: Vec<String>,
    #[serde(default)]
    pub skills: Vec<BundleSkill>,
    #[serde(default)]
    pub plugins: Vec<BundlePlugin>,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Vec<ChatMessage>>,
}

/// What a bundle contains, for the import dialog and export confirmation.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct BundleInfo {
    pub name: String,
    pub persona: String,
    pub has_system_prompt: bool,
    pub created: u32,
    pub app: String,
    pub memory: u32,
    pub skills: Vec<String>,
    pub plugins: Vec<String>,
    /// Tool toggles the bundle asks for (always left off on import).
    pub tools: Vec<String>,
    pub transcript: u32,
}

/// What an import actually wrote.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct BundleReport {
    pub identity: bool,
    pub memory_added: u32,
    pub memory_skipped: u32,
    pub skills: Vec<String>,
    pub plugins: Vec<String>,
    /// Archive id of the imported transcript, when one was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// Requested tools that stay disabled until enabled by hand.
    pub tools_off: Vec<String>,
}

fn now_secs() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

/// Human labels for the assistant's system-control toggles.
pub const TOOL_LABELS: &[(&str, &str)] = &[
    ("tool_clipboard", "Clipboard"),
    ("tool_windows", "Window control"),
    ("tool_screen", "Screen capture"),
    ("tool_input", "Input control"),
    ("tool_uia", "Accessibility (uia)"),
    ("tool_browser", "Browser control"),
];

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn valid_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.contains(['/', '\\'])
        && !name.contains("..")
        && !name.starts_with('.')
}

/// The body of a SKILL.md, past its frontmatter block.
fn skill_body(text: &str) -> String {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return text.trim().to_string();
    }
    let mut body = String::new();
    let mut in_body = false;
    for line in lines {
        if !in_body {
            if line.trim() == "---" {
                in_body = true;
            }
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    body.trim().to_string()
}

/// A free `<dir>/<name>` path, suffixing `-2`, `-3`, … on collisions.
fn free_dir(dir: &Path, name: &str, separator: char) -> PathBuf {
    let mut candidate = dir.join(name);
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{name}{separator}{n}"));
        n += 1;
    }
    candidate
}

/// Gather everything the selected sections need from the local setup.
pub fn build(
    base: &Path,
    config: &crate::config::AssistantConfig,
    include: &BundleInclude,
    transcript: Option<Vec<ChatMessage>>,
) -> Result<Bundle> {
    let mut memory = Vec::new();
    if include.memory {
        if let Some(path) = crate::assistant::memory_path() {
            if let Ok(text) = std::fs::read_to_string(path) {
                memory = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .take(MEMORY_LINES_CAP)
                    .map(str::to_string)
                    .collect();
            }
        }
    }
    let mut skills = Vec::new();
    if include.skills {
        for skill in harness::skills::discover(&[base.join("skills")]).into_iter().take(SKILLS_CAP) {
            let Ok(text) = std::fs::read_to_string(&skill.path) else {
                continue;
            };
            skills.push(BundleSkill {
                name: skill.name,
                description: skill.description,
                body: skill_body(&text),
            });
        }
    }
    let mut plugins = Vec::new();
    if include.plugins {
        let (found, _) = harness::plugins::discover(&[("global".to_string(), base.join("plugins"))]);
        for plugin in found.into_iter().take(PLUGINS_CAP) {
            let Ok(text) = std::fs::read_to_string(plugin.dir.join("plugin.json")) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str(&text) else {
                continue;
            };
            let mut files = BTreeMap::new();
            let mut total = 0usize;
            if let Ok(entries) = std::fs::read_dir(&plugin.dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if name == "plugin.json" || !path.is_file() || !plain_file_name(name) {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    if text.len() > PLUGIN_FILE_CAP || total + text.len() > PLUGIN_TOTAL_CAP {
                        continue;
                    }
                    total += text.len();
                    files.insert(name.to_string(), text);
                }
            }
            plugins.push(BundlePlugin {
                name: plugin.name,
                manifest,
                files,
            });
        }
    }
    let mut tools = BTreeMap::new();
    for (key, _) in TOOL_LABELS {
        let on = match *key {
            "tool_clipboard" => config.tool_clipboard,
            "tool_windows" => config.tool_windows,
            "tool_screen" => config.tool_screen,
            "tool_input" => config.tool_input,
            "tool_uia" => config.tool_uia,
            "tool_browser" => config.tool_browser,
            _ => false,
        };
        tools.insert((*key).to_string(), on);
    }
    Ok(Bundle {
        format: BUNDLE_FORMAT.to_string(),
        version: BUNDLE_VERSION,
        kind: "assistant".to_string(),
        created: now_secs(),
        app: env!("CARGO_PKG_VERSION").to_string(),
        name: config.name.clone(),
        persona: config.persona.clone(),
        system_prompt: config.system_prompt.clone().filter(|p| !p.trim().is_empty()),
        generation: BundleGeneration {
            temperature: config.temperature,
            top_p: config.top_p,
            repeat_penalty: config.repeat_penalty,
            reasoning_effort: config.reasoning_effort.clone(),
        },
        memory,
        skills,
        plugins,
        tools,
        transcript: transcript.filter(|_| include.transcript).map(|mut t| {
            if t.len() > TRANSCRIPT_CAP {
                t.truncate(TRANSCRIPT_CAP);
            }
            t
        }),
    })
}

/// Read and validate a bundle file.
pub fn read_bundle(path: &Path) -> Result<Bundle> {
    let text = std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let bundle: Bundle = serde_json::from_str(&text).context("Not a werk bundle")?;
    if bundle.format != BUNDLE_FORMAT {
        bail!("Not a werk bundle (format '{}')", bundle.format);
    }
    if bundle.version > BUNDLE_VERSION {
        bail!(
            "Bundle version {} is newer than this app supports ({BUNDLE_VERSION})",
            bundle.version
        );
    }
    if bundle.kind != "assistant" {
        bail!("Unsupported bundle kind '{}'", bundle.kind);
    }
    Ok(bundle)
}

pub fn write_bundle(path: &Path, bundle: &Bundle) -> Result<()> {
    let text = serde_json::to_string_pretty(bundle)?;
    std::fs::write(path, text).with_context(|| format!("Cannot write {}", path.display()))?;
    Ok(())
}

fn requested_tools(bundle: &Bundle) -> Vec<String> {
    TOOL_LABELS
        .iter()
        .filter(|(key, _)| bundle.tools.get(*key).copied().unwrap_or(false))
        .map(|(_, label)| (*label).to_string())
        .collect()
}

pub fn info(bundle: &Bundle) -> BundleInfo {
    BundleInfo {
        name: bundle.name.clone(),
        persona: bundle.persona.chars().take(400).collect(),
        has_system_prompt: bundle.system_prompt.as_deref().map(|p| !p.trim().is_empty()).unwrap_or(false),
        created: bundle.created,
        app: bundle.app.clone(),
        memory: bundle.memory.len() as u32,
        skills: bundle.skills.iter().map(|s| s.name.clone()).collect(),
        plugins: bundle.plugins.iter().map(|p| p.name.clone()).collect(),
        tools: requested_tools(bundle),
        transcript: bundle.transcript.as_ref().map(|t| t.len() as u32).unwrap_or(0),
    }
}

pub fn inspect(path: &Path) -> Result<BundleInfo> {
    Ok(info(&read_bundle(path)?))
}

/// Write the selected sections into the local setup. Never overwrites: skills
/// and plugins land under a suffixed name when one already exists. Returns
/// what was written; the caller applies identity settings.
pub fn import(base: &Path, bundle: &Bundle, include: &BundleInclude) -> Result<BundleReport> {
    let mut report = BundleReport {
        identity: include.identity,
        memory_added: 0,
        memory_skipped: 0,
        skills: Vec::new(),
        plugins: Vec::new(),
        transcript: None,
        tools_off: requested_tools(bundle),
    };
    if include.memory {
        let (added, skipped) = import_memory(base, &bundle.memory)?;
        report.memory_added = added;
        report.memory_skipped = skipped;
    }
    if include.skills {
        for skill in bundle.skills.iter().take(SKILLS_CAP) {
            if !valid_skill_name(&skill.name) || skill.body.trim().is_empty() {
                continue;
            }
            if skill.body.len() > harness::skills::SKILL_BODY_CAP {
                continue;
            }
            let dir = free_dir(&base.join("skills"), &skill.name, '-');
            std::fs::create_dir_all(&dir)?;
            let text = format!(
                "---\nname: {}\ndescription: {}\n---\n\n{}\n",
                skill.name,
                skill.description.trim(),
                skill.body.trim()
            );
            std::fs::write(dir.join("SKILL.md"), text)?;
            report
                .skills
                .push(dir.file_name().and_then(|n| n.to_str()).unwrap_or(&skill.name).to_string());
        }
    }
    if include.plugins {
        for plugin in bundle.plugins.iter().take(PLUGINS_CAP) {
            if !valid_plugin_name(&plugin.name) {
                continue;
            }
            let dir = free_dir(&base.join("plugins"), &plugin.name, '_');
            std::fs::create_dir_all(&dir)?;
            let mut manifest: harness::plugins::PluginManifest =
                serde_json::from_value(plugin.manifest.clone())
                    .with_context(|| format!("Plugin '{}' has an invalid manifest", plugin.name))?;
            manifest.name = plugin.name.clone();
            // Imported code is untrusted: always ask before running it.
            manifest.approval = true;
            harness::plugins::validate(&dir, "global", manifest.clone())?;
            let mut total = 0usize;
            for (name, content) in &plugin.files {
                if !plain_file_name(name) || content.len() > PLUGIN_FILE_CAP {
                    continue;
                }
                if total + content.len() > PLUGIN_TOTAL_CAP {
                    break;
                }
                total += content.len();
                std::fs::write(dir.join(name), content)?;
            }
            std::fs::write(dir.join("plugin.json"), serde_json::to_string_pretty(&manifest)?)?;
            report
                .plugins
                .push(dir.file_name().and_then(|n| n.to_str()).unwrap_or(&plugin.name).to_string());
        }
    }
    if include.transcript {
        if let Some(messages) = bundle.transcript.as_ref().filter(|t| !t.is_empty()) {
            let mut messages = messages.clone();
            if messages.len() > TRANSCRIPT_CAP {
                messages.truncate(TRANSCRIPT_CAP);
            }
            report.transcript =
                Some(crate::chat::archive_imported_transcript(messages).map_err(|e| anyhow!(e))?);
        }
    }
    Ok(report)
}

/// Append memory lines that are not already present, up to the file cap.
fn import_memory(base: &Path, lines: &[String]) -> Result<(u32, u32)> {
    let path = base.join("assistant").join("MEMORY.md");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut seen: Vec<String> = existing
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect();
    let mut out = existing.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    let (mut added, mut skipped) = (0u32, 0u32);
    for line in lines.iter().take(MEMORY_LINES_CAP) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if seen.contains(&key) || out.len() + trimmed.len() + 1 > harness::memory::MEMORY_FILE_CAP {
            skipped += 1;
            continue;
        }
        seen.push(key);
        out.push_str(trimmed);
        out.push('\n');
        added += 1;
    }
    if added > 0 {
        std::fs::write(&path, out)?;
    }
    Ok((added, skipped))
}

// ── Commands ──────────────────────────────────────────────────────────────

/// Export the assistant setup to a `.werk` file.
#[tauri::command]
#[specta::specta]
pub async fn assistant_bundle_export(
    path: String,
    include: BundleInclude,
    state: tauri::State<'_, crate::AppState>,
) -> Result<BundleInfo, String> {
    let base = crate::assistant_tools::werk_base().map_err(|e| e.to_string())?;
    let config = state.config.lock().unwrap().assistant.clone();
    let transcript = include
        .transcript
        .then(crate::chat::assistant_transcript_messages)
        .filter(|t| !t.is_empty());
    let bundle = build(&base, &config, &include, transcript).map_err(|e| e.to_string())?;
    write_bundle(Path::new(&path), &bundle).map_err(|e| e.to_string())?;
    Ok(info(&bundle))
}

/// What a bundle file contains, for the import dialog.
#[tauri::command]
#[specta::specta]
pub async fn assistant_bundle_inspect(path: String) -> Result<BundleInfo, String> {
    inspect(Path::new(&path)).map_err(|e| e.to_string())
}

/// Import the selected parts of a bundle. Tool toggles in the bundle are
/// never enabled; skills and plugins land under suffixed names on collisions.
#[tauri::command]
#[specta::specta]
pub async fn assistant_bundle_import(
    path: String,
    include: BundleInclude,
    state: tauri::State<'_, crate::AppState>,
) -> Result<BundleReport, String> {
    let base = crate::assistant_tools::werk_base().map_err(|e| e.to_string())?;
    let bundle = read_bundle(Path::new(&path)).map_err(|e| e.to_string())?;
    let report = import(&base, &bundle, &include).map_err(|e| e.to_string())?;
    if include.identity {
        let mut config = state.config.lock().unwrap();
        config.assistant.name = bundle.name.clone();
        config.assistant.persona = bundle.persona.clone();
        config.assistant.system_prompt = bundle
            .system_prompt
            .clone()
            .filter(|p| !p.trim().is_empty());
        config.assistant.temperature = bundle.generation.temperature;
        config.assistant.top_p = bundle.generation.top_p;
        config.assistant.repeat_penalty = bundle.generation.repeat_penalty;
        config.assistant.reasoning_effort = bundle.generation.reasoning_effort.clone();
        config.save().map_err(|e| e.to_string())?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("werk-bundle-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Bundle {
        Bundle {
            format: BUNDLE_FORMAT.to_string(),
            version: BUNDLE_VERSION,
            kind: "assistant".to_string(),
            created: 1,
            app: "0.0.0".to_string(),
            name: "Ada".to_string(),
            persona: "Dry wit.".to_string(),
            system_prompt: Some("Be brief.".to_string()),
            generation: BundleGeneration::default(),
            memory: vec![
                "- [2026-01-01] user: likes tea".to_string(),
                "- [2026-01-01] user: likes tea".to_string(),
            ],
            skills: vec![BundleSkill {
                name: "tea-notes".to_string(),
                description: "How to note teas".to_string(),
                body: "Write them down.".to_string(),
            }],
            plugins: vec![BundlePlugin {
                name: "tea_lookup".to_string(),
                manifest: serde_json::json!({
                    "name": "tea_lookup",
                    "description": "Look up a tea",
                    "command": ["python", "run.py"],
                    "approval": false
                }),
                files: [("run.py".to_string(), "print('tea')".to_string())]
                    .into_iter()
                    .collect(),
            }],
            tools: [("tool_windows".to_string(), true)].into_iter().collect(),
            transcript: None,
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_base("round");
        let path = dir.join("ada.werk");
        write_bundle(&path, &sample()).unwrap();
        let loaded = read_bundle(&path).unwrap();
        assert_eq!(loaded.name, "Ada");
        assert_eq!(loaded.skills.len(), 1);
        let info = info(&loaded);
        assert_eq!(info.tools, vec!["Window control".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_foreign_files() {
        let dir = temp_base("foreign");
        let path = dir.join("bad.werk");
        std::fs::write(&path, "{\"format\":\"something-else\",\"version\":1,\"kind\":\"assistant\",\"name\":\"x\"}").unwrap();
        assert!(read_bundle(&path).is_err());
        std::fs::write(&path, "not json").unwrap();
        assert!(read_bundle(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_guards_collisions_and_tool_toggles() {
        let dir = temp_base("import");
        std::fs::create_dir_all(dir.join("skills").join("tea-notes")).unwrap();
        std::fs::write(
            dir.join("skills").join("tea-notes").join("SKILL.md"),
            "---\nname: tea-notes\ndescription: old\n---\n\nold\n",
        )
        .unwrap();
        let include = BundleInclude {
            identity: true,
            memory: true,
            skills: true,
            plugins: true,
            transcript: false,
        };
        let report = import(&dir, &sample(), &include).unwrap();
        // The existing skill was kept; the import landed under a suffix.
        assert_eq!(report.skills, vec!["tea-notes-2".to_string()]);
        assert!(dir.join("skills").join("tea-notes-2").join("SKILL.md").is_file());
        // Memory dedupes the repeated line.
        assert_eq!(report.memory_added, 1);
        assert_eq!(report.memory_skipped, 1);
        // Requested tools are reported but never enabled.
        assert_eq!(report.tools_off, vec!["Window control".to_string()]);
        // Imported plugins always ask for approval.
        let manifest: harness::plugins::PluginManifest =
            serde_json::from_str(&std::fs::read_to_string(dir.join("plugins").join("tea_lookup").join("plugin.json")).unwrap()).unwrap();
        assert!(manifest.approval);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_skips_unsafe_plugin_files() {
        let dir = temp_base("unsafe");
        let mut bundle = sample();
        bundle.plugins[0].files.insert("../evil.sh".to_string(), "boom".to_string());
        bundle.plugins[0].files.insert("ok.py".to_string(), "print('ok')".to_string());
        let include = BundleInclude {
            identity: false,
            memory: false,
            skills: false,
            plugins: true,
            transcript: false,
        };
        import(&dir, &bundle, &include).unwrap();
        let plugin = dir.join("plugins").join("tea_lookup");
        assert!(plugin.join("ok.py").is_file());
        // The traversal name was skipped, so nothing landed beside the plugin.
        assert!(!dir.join("plugins").join("evil.sh").exists());
        assert!(!dir.join("evil.sh").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skill_body_drops_frontmatter() {
        let text = "---\nname: x\ndescription: y\n---\n\nDo the thing.\n";
        assert_eq!(skill_body(text), "Do the thing.");
        assert_eq!(skill_body("plain text"), "plain text");
    }
}
