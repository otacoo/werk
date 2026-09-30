//! File-defined plugin tools: a folder with `plugin.json` plus an executable.
//!
//! ```text
//! my_plugin/
//!   plugin.json      <- { name, description, parameters, command, approval }
//!   run.py           <- reads {"args": {...}} from stdin, prints the result
//! ```
//!
//! `command` runs with the project directory as cwd. Relative script paths
//! resolve against the plugin folder; bare names resolve via PATH. Stdout
//! (trimmed) is the tool result. Rediscovered every run; project beats
//! global on conflicts. Orchestrator-only in M7 (subagent allowlists omit it).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::permissions::ApprovalKey;
use crate::tools::Tool;

pub const PLUGIN_OUTPUT_CAP: usize = 30_000;
pub const PLUGIN_TIMEOUT_SECS: u64 = 120;

fn default_parameters() -> Value {
    json!({"type": "object", "properties": {}})
}

fn default_approval() -> bool {
    true
}

/// On-disk manifest inside the plugin folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_parameters")]
    pub parameters: Value,
    #[serde(default)]
    pub command: Vec<String>,
    /// False runs free; true (default) asks approval first.
    #[serde(default = "default_approval")]
    pub approval: bool,
}

#[derive(Debug, Clone)]
pub struct PluginDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub command: Vec<String>,
    pub approval: bool,
    pub scope: String,
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginError {
    pub dir: String,
    pub error: String,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn validate(dir: &Path, scope: &str, manifest: PluginManifest) -> Result<PluginDef> {
    if !valid_name(&manifest.name) {
        bail!(
            "invalid plugin name '{}' (lowercase, digits, _; max 64)",
            manifest.name
        );
    }
    if manifest.command.is_empty() || manifest.command.iter().any(|c| c.trim().is_empty()) {
        bail!("plugin '{}' needs a non-empty 'command' array", manifest.name);
    }
    if !manifest.parameters.is_object() {
        bail!("plugin '{}' 'parameters' must be a JSON object", manifest.name);
    }
    Ok(PluginDef {
        name: manifest.name,
        description: manifest.description,
        parameters: manifest.parameters,
        command: manifest.command,
        approval: manifest.approval,
        scope: scope.to_string(),
        dir: dir.to_path_buf(),
    })
}

/// Discover under scoped roots; later scopes win. Returns plugins + errors
/// so the UI can show broken manifests to their authors.
pub fn discover(scoped_roots: &[(String, PathBuf)]) -> (Vec<PluginDef>, Vec<PluginError>) {
    let mut out: Vec<PluginDef> = Vec::new();
    let mut errors: Vec<PluginError> = Vec::new();
    for (scope, root) in scoped_roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() || !dir.join("plugin.json").is_file() {
                continue;
            }
            let text = match std::fs::read_to_string(dir.join("plugin.json")) {
                Ok(t) => t,
                Err(e) => {
                    errors.push(PluginError {
                        dir: dir.to_string_lossy().to_string(),
                        error: format!("cannot read plugin.json: {e}"),
                    });
                    continue;
                }
            };
            let manifest: PluginManifest = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    errors.push(PluginError {
                        dir: dir.to_string_lossy().to_string(),
                        error: format!("invalid plugin.json: {e}"),
                    });
                    continue;
                }
            };
            match validate(&dir, scope, manifest) {
                Ok(def) => {
                    out.retain(|d| d.name != def.name);
                    out.push(def);
                }
                Err(e) => errors.push(PluginError {
                    dir: dir.to_string_lossy().to_string(),
                    error: e.to_string(),
                }),
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    (out, errors)
}

pub struct PluginTool {
    def: PluginDef,
    workdir: PathBuf,
}

impl PluginTool {
    pub fn new(def: PluginDef, workdir: PathBuf) -> Self {
        Self { def, workdir }
    }

    /// Relative script paths resolve against the plugin folder; bare
    /// command names and project paths pass through untouched.
    fn resolve_arg(&self, arg: &str) -> String {
        let p = Path::new(arg);
        if p.is_absolute() {
            return arg.to_string();
        }
        let candidate = self.def.dir.join(p);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
        arg.to_string()
    }
}

impl Tool for PluginTool {
    fn name(&self) -> String {
        self.def.name.clone()
    }

    fn description(&self) -> String {
        if self.def.description.is_empty() {
            format!("Plugin tool '{}' (runs a local command).", self.def.name)
        } else {
            format!(
                "{} (plugin: runs a local command in the project directory).",
                self.def.description
            )
        }
    }

    fn parameters(&self) -> Value {
        self.def.parameters.clone()
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        if self.def.approval {
            Some(ApprovalKey { tool: self.def.name.clone(), command: None })
        } else {
            None
        }
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let argv: Vec<String> = self.def.command.iter().map(|c| self.resolve_arg(c)).collect();
        let mut cmd = std::process::Command::new(&argv[0]);
        if argv.len() > 1 {
            cmd.args(&argv[1..]);
        }
        cmd.current_dir(&self.workdir)
            .stdin(std::process::Stdio::piped())
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
            .with_context(|| format!("Plugin '{}': failed to spawn {:?}", self.def.name, argv))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(json!({"args": args}).to_string().as_bytes());
        }
        let stdout = child.stdout.take().context("no stdout pipe")?;
        let stderr = child.stderr.take().context("no stderr pipe")?;
        let out_handle = std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = std::io::BufReader::new(stdout).read_to_end(&mut buf);
            buf
        });
        let err_handle = std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = std::io::BufReader::new(stderr).read_to_end(&mut buf);
            buf
        });
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(PLUGIN_TIMEOUT_SECS);
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
                        let _ = child.kill();
                        let _ = child.wait();
                        timed_out = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => bail!("Plugin '{}': wait failed: {e}", self.def.name),
            }
        }
        let out_bytes = out_handle.join().unwrap_or_default();
        let err_bytes = err_handle.join().unwrap_or_default();
        let mut text = String::from_utf8_lossy(&out_bytes).trim().to_string();
        let err_text = String::from_utf8_lossy(&err_bytes).trim().to_string();
        if timed_out {
            bail!(
                "Plugin '{}' timed out after {PLUGIN_TIMEOUT_SECS}s and was killed{}",
                self.def.name,
                if err_text.is_empty() { String::new() } else { format!(" (stderr: {err_text})") }
            );
        }
        if !matches!(status, Some(s) if s.success()) {
            let code = status
                .and_then(|s| s.code())
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".to_string());
            bail!(
                "Plugin '{}' exited with code {code}{}",
                self.def.name,
                if err_text.is_empty() { String::new() } else { format!(" (stderr: {err_text})") }
            );
        }
        if text.chars().count() > PLUGIN_OUTPUT_CAP {
            let n = text.chars().count();
            text = text.chars().take(PLUGIN_OUTPUT_CAP).collect();
            text.push_str(&format!("\n[truncated — {} more characters]", n - PLUGIN_OUTPUT_CAP));
        }
        if text.is_empty() {
            text = "(plugin produced no output)".to_string();
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("werk-plugins-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_plugin(root: &Path, folder: &str, manifest: &str) -> PathBuf {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();
        dir
    }

    #[test]
    fn discovers_valid_plugins_project_beats_global() {
        let global = plugin_root("g");
        let project = plugin_root("p");
        write_plugin(&global, "echo", r#"{"name": "echo", "description": "g", "command": ["echo"]}"#);
        write_plugin(&project, "echo", r#"{"name": "echo", "description": "p", "command": ["echo"]}"#);
        write_plugin(&project, "broken", r#"{"name": "Broken Name"}"#);
        std::fs::create_dir_all(project.join("no-manifest")).unwrap();
        let (defs, errors) = discover(&[
            ("global".to_string(), global.clone()),
            ("project".to_string(), project.clone()),
        ]);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].scope, "project");
        assert_eq!(defs[0].description, "p");
        assert!(defs[0].approval, "approval defaults on");
        assert_eq!(errors.len(), 1);
        let _ = std::fs::remove_dir_all(&global);
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn rejects_bad_manifests() {
        let root = plugin_root("reject");
        write_plugin(&root, "no-command", r#"{"name": "nocmd"}"#);
        write_plugin(&root, "bad-params", r#"{"name": "bp", "command": ["echo"], "parameters": []}"#);
        write_plugin(&root, "not-json", r#"{"name": "#);
        let (defs, errors) = discover(&[("global".to_string(), root.clone())]);
        assert!(defs.is_empty());
        assert_eq!(errors.len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn co_located_script_resolves_against_plugin_dir() {
        let root = plugin_root("resolve");
        let dir = write_plugin(&root, "mine", r#"{"name": "mine", "command": ["run.py"]}"#);
        std::fs::write(dir.join("run.py"), b"print('hi')").unwrap();
        let (defs, _) = discover(&[("global".to_string(), root.clone())]);
        let tool = PluginTool::new(defs[0].clone(), root.clone());
        assert_eq!(
            tool.resolve_arg("run.py"),
            dir.join("run.py").to_string_lossy().to_string()
        );
        assert_eq!(tool.resolve_arg("python"), "python");
        assert_eq!(tool.resolve_arg("data/in.txt"), "data/in.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plugin_tool_round_trips_stdin_stdout() {
        let root = plugin_root("exec");
        #[cfg(target_os = "windows")]
        let command = r#"{"name": "catin", "command": ["powershell", "-NoProfile", "-Command", "$input | Out-String"], "approval": false}"#;
        #[cfg(not(target_os = "windows"))]
        let command = r#"{"name": "catin", "command": ["cat"], "approval": false}"#;
        let dir = write_plugin(&root, "catin", command);
        let (defs, _) = discover(&[("global".to_string(), root.clone())]);
        let tool = PluginTool::new(defs[0].clone(), dir.clone());
        assert!(tool.approval_key(&json!({})).is_none());
        let out = tool.execute(&json!({"hello": "world"})).unwrap();
        assert!(out.contains("hello"), "{out}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plugin_tool_reports_nonzero_exit() {
        let root = plugin_root("exit");
        #[cfg(target_os = "windows")]
        let command = r#"{"name": "failer", "command": ["powershell", "-NoProfile", "-Command", "exit 3"]}"#;
        #[cfg(not(target_os = "windows"))]
        let command = r#"{"name": "failer", "command": ["sh", "-c", "exit 3"]}"#;
        write_plugin(&root, "failer", command);
        let (defs, _) = discover(&[("global".to_string(), root.clone())]);
        let tool = PluginTool::new(defs[0].clone(), root.clone());
        let err = tool.execute(&json!({})).unwrap_err().to_string();
        assert!(err.contains("code 3"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
