//! Assistant-authored capabilities: skills, plugins, and MCP servers.
//! Writes land in the shared global folders, so the agent profile sees them.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use harness::permissions::ApprovalKey;
use harness::tools::Tool;

pub fn werk_base() -> Result<PathBuf> {
    crate::config::data_dir()
        .map(|d| d.join("werk"))
        .ok_or_else(|| anyhow!("Cannot find the data directory"))
}

fn valid_slug(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn req<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("'{name}' is required"))
}

/// Save a reusable procedure as a skill.
pub struct SkillWriteTool {
    base: PathBuf,
}

impl SkillWriteTool {
    /// `base` is the shared werk data folder (`<data>/werk`).
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }
}

impl Tool for SkillWriteTool {
    fn name(&self) -> String {
        "skill_write".to_string()
    }

    fn description(&self) -> String {
        "Create or update a skill: a reusable procedure saved as SKILL.md in the shared \
         skills folder, listed in the prompt and loadable with the skill tool. Pass \
         overwrite=true to replace an existing skill."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Lowercase slug (a-z, 0-9, -, _)." },
                "description": { "type": "string", "description": "One line shown in the skill list." },
                "body": { "type": "string", "description": "Markdown instructions." },
                "overwrite": { "type": "boolean", "description": "Replace an existing skill." }
            },
            "required": ["name", "description", "body"]
        })
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let name = req(args, "name")?.to_lowercase();
        if !valid_slug(&name) {
            bail!("Invalid skill name '{name}' (lowercase letters, digits, - or _; max 64)");
        }
        let description = req(args, "description")?;
        let body = req(args, "body")?;
        let overwrite = args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let dir = self.base.join("skills").join(&name);
        let path = dir.join("SKILL.md");
        if path.exists() && !overwrite {
            bail!("Skill '{name}' already exists (pass overwrite: true to replace it)");
        }
        std::fs::create_dir_all(&dir)?;
        let text = format!(
            "---\nname: {name}\ndescription: {}\n---\n\n{}\n",
            description.trim(),
            body.trim()
        );
        std::fs::write(&path, text)?;
        Ok(format!(
            "Saved skill '{name}'. It is listed in the prompt on the next run."
        ))
    }
}

/// Create a local-command plugin for the agent.
pub struct PluginWriteTool {
    base: PathBuf,
}

impl PluginWriteTool {
    /// `base` is the shared werk data folder (`<data>/werk`).
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }
}

impl Tool for PluginWriteTool {
    fn name(&self) -> String {
        "plugin_write".to_string()
    }

    fn description(&self) -> String {
        "Create a Werk plugin: a local command exposed to the agent as a tool. Writes \
         plugin.json (and an optional co-located script) in the shared plugins folder; the \
         command reads {\"args\": {...}} from stdin and prints the result. Pass overwrite=true \
         to replace an existing plugin."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Lowercase slug (a-z, 0-9, _)." },
                "description": { "type": "string" },
                "command": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Argv, e.g. [\"python\", \"run.py\"]. Relative script names resolve in the plugin folder."
                },
                "parameters": { "type": "object", "description": "JSON Schema for the tool arguments." },
                "script": { "type": "string", "description": "Optional script contents, saved next to plugin.json." },
                "script_name": { "type": "string", "description": "File name for `script`, e.g. run.py." },
                "approval": { "type": "boolean", "description": "Ask the user before each run (default true)." },
                "overwrite": { "type": "boolean" }
            },
            "required": ["name", "description", "command"]
        })
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let name = req(args, "name")?.to_lowercase();
        if !valid_slug(&name) || name.contains('-') {
            bail!("Invalid plugin name '{name}' (lowercase letters, digits, _; max 64)");
        }
        let command: Vec<String> = args
            .get("command")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if command.is_empty() {
            bail!("'command' must be a non-empty array of strings");
        }
        let overwrite = args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let dir = self.base.join("plugins").join(&name);
        let manifest_path = dir.join("plugin.json");
        if manifest_path.exists() && !overwrite {
            bail!("Plugin '{name}' already exists (pass overwrite: true to replace it)");
        }
        std::fs::create_dir_all(&dir)?;
        if let Some(script) = args.get("script").and_then(Value::as_str) {
            let file = req(args, "script_name")?;
            if file.contains("..") || file.contains('/') || file.contains('\\') {
                bail!("'script_name' must be a plain file name");
            }
            std::fs::write(dir.join(file), script)?;
        }
        let parameters = args
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
        if !parameters.is_object() {
            bail!("'parameters' must be a JSON object");
        }
        let manifest = harness::plugins::PluginManifest {
            name: name.clone(),
            description: req(args, "description")?.to_string(),
            parameters,
            command,
            approval: args
                .get("approval")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        };
        harness::plugins::validate(&dir, "global", manifest.clone())?;
        std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
        Ok(format!(
            "Saved plugin '{name}'. The agent sees it as a tool on the next run."
        ))
    }
}

/// Manage MCP servers (Cursor-compatible `mcp.json`).
pub struct McpTool {
    app: tauri::AppHandle,
}

impl McpTool {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl Tool for McpTool {
    fn name(&self) -> String {
        "mcp".to_string()
    }

    fn description(&self) -> String {
        "Manage MCP servers in the shared mcp.json: `list` them, `add` one (name + command + \
         args + env), or `remove` one by name. Added servers start on the next run."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "add", "remove"] },
                "name": { "type": "string" },
                "command": { "type": "string", "description": "Executable, e.g. npx or uvx." },
                "args": { "type": "array", "items": { "type": "string" } },
                "env": { "type": "object", "description": "Extra environment variables." }
            },
            "required": ["action"]
        })
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }

    fn execute(&self, args: &Value) -> Result<String> {
        use tauri::Manager;
        let state = self
            .app
            .try_state::<crate::AppState>()
            .ok_or_else(|| anyhow!("App state unavailable"))?;
        let path = crate::mcp::mcp_config_path().map_err(|e| anyhow!("{e}"))?;
        crate::mcp::ensure_defaults(&path).map_err(|e| anyhow!("{e}"))?;
        let cfg = crate::mcp::load_at(&path).map_err(|e| anyhow!("{e}"))?;
        let disabled = state.config.lock().unwrap().mcp_disabled.clone();
        let mut entries = crate::mcp::entries_from_config(&cfg);
        for e in entries.iter_mut() {
            e.enabled = !disabled.iter().any(|d| d == &e.name);
        }
        match args.get("action").and_then(Value::as_str).unwrap_or("") {
            "list" => {
                if entries.is_empty() {
                    return Ok("No MCP servers configured.".to_string());
                }
                let mut out = String::new();
                for e in &entries {
                    out.push_str(&format!(
                        "- {}: {} {}{}\n",
                        e.name,
                        e.command,
                        e.args.join(" "),
                        if e.enabled { "" } else { " (disabled)" }
                    ));
                }
                Ok(out)
            }
            "add" => {
                let name = req(args, "name")?.to_string();
                let command = req(args, "command")?.to_string();
                let args_v: Vec<String> = args
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let env: std::collections::HashMap<String, String> = args
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|o| {
                        o.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                entries.retain(|e| e.name != name);
                entries.push(crate::mcp::McpServerEntry {
                    name: name.clone(),
                    command,
                    args: args_v,
                    env,
                    cwd: None,
                    timeout_ms: None,
                    enabled: true,
                });
                crate::mcp::save_at(&entries, &path).map_err(|e| anyhow!("{e}"))?;
                state.mcp_agent.shutdown();
                let mut config = state.config.lock().unwrap();
                config.mcp_disabled.retain(|d| d != &name);
                config.save().map_err(|e| anyhow!("{e}"))?;
                Ok(format!("Added MCP server '{name}'. It starts on the next run."))
            }
            "remove" => {
                let name = req(args, "name")?.to_string();
                let before = entries.len();
                entries.retain(|e| e.name != name);
                if entries.len() == before {
                    bail!("No MCP server named '{name}'");
                }
                crate::mcp::save_at(&entries, &path).map_err(|e| anyhow!("{e}"))?;
                state.mcp_agent.shutdown();
                let mut config = state.config.lock().unwrap();
                config.mcp_disabled.retain(|d| d != &name);
                config.save().map_err(|e| anyhow!("{e}"))?;
                Ok(format!("Removed MCP server '{name}'."))
            }
            other => bail!("Unknown action '{other}' (expected list/add/remove)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("werk-atools-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn skill_write_round_trips_through_discovery() {
        let base = base("skill");
        let tool = SkillWriteTool::new(base.clone());
        let out = tool
            .execute(&json!({
                "name": "deploy",
                "description": "how to deploy",
                "body": "Run the deploy script."
            }))
            .unwrap();
        assert!(out.contains("deploy"));
        // The written file is a valid skill for the real discovery code.
        let found = harness::skills::discover(&[base.join("skills")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "deploy");
        assert_eq!(found[0].description, "how to deploy");
        // Existing skills are protected unless overwrite is set.
        assert!(tool
            .execute(&json!({"name": "deploy", "description": "x", "body": "y"}))
            .is_err());
        tool.execute(&json!({
            "name": "deploy",
            "description": "x",
            "body": "y",
            "overwrite": true
        }))
        .unwrap();
        assert!(tool
            .execute(&json!({"name": "Bad Name", "description": "x", "body": "y"}))
            .is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn plugin_write_round_trips_through_discovery() {
        let base = base("plugin");
        let tool = PluginWriteTool::new(base.clone());
        tool.execute(&json!({
            "name": "hello_world",
            "description": "Say hello.",
            "command": ["python", "run.py"],
            "script": "print('hi')",
            "script_name": "run.py",
            "approval": false,
            "parameters": {"type": "object", "properties": {"who": {"type": "string"}}}
        }))
        .unwrap();
        let (plugins, errors) =
            harness::plugins::discover(&[("global".to_string(), base.join("plugins"))]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "hello_world");
        assert!(!plugins[0].approval);
        assert!(base
            .join("plugins")
            .join("hello_world")
            .join("run.py")
            .is_file());
        // Missing command and traversal script names are rejected.
        assert!(tool
            .execute(&json!({"name": "x", "description": "d", "command": []}))
            .is_err());
        assert!(tool
            .execute(&json!({
                "name": "y",
                "description": "d",
                "command": ["python"],
                "script": "s",
                "script_name": "../evil.py"
            }))
            .is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
