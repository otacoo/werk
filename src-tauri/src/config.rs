//! App config: one versioned struct, atomic saves, migrate on load.
//! Every field has a serde default so future fields add cleanly.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Schema version; bump when a migration is added below.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ManagedRuntime {
    pub build: u32,
    pub tag_name: String,
    pub backend_id: String,
    pub backend_label: String,
    pub asset_name: String,
    /// Subdirectory name under the runtimes base dir
    pub dir_name: String,
    /// Unix seconds (u32: exact in JSON, good until 2106).
    pub installed_at: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct CustomRuntime {
    pub label: String,
    pub binary_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, specta::Type)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActiveRuntime {
    Managed {
        build: u32,
        #[serde(default)]
        backend_id: String,
    },
    Custom {
        index: u32,
    },
    #[default]
    None,
}

/// How the harness gets its model: one local model, a local router, or an
/// external OpenAI-compatible API (no local server).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ServerMode {
    #[default]
    Single,
    Router,
    External,
}

/// A named system prompt the user can swap in from Settings.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SystemPromptPreset {
    pub name: String,
    pub prompt: String,
}

/// Chat profile: the coding agent or a roleplay character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatProfile {
    #[default]
    Agent,
    Roleplay,
}

/// Roleplay profile state: active card, user persona, greeting index, and
/// per-request sampling overrides (None = server defaults).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RoleplayConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    #[serde(default)]
    pub greeting: u32,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub user_description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
}

impl Default for RoleplayConfig {
    fn default() -> Self {
        Self {
            card_id: None,
            greeting: 0,
            user_name: String::new(),
            user_description: String::new(),
            temperature: None,
            top_p: None,
            repeat_penalty: None,
        }
    }
}

/// An external OpenAI-compatible provider (cloud API or another local
/// server). External API mode targets it as `provider-id:model-id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct Provider {
    pub id: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    /// Declared context window for its models; None = unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

impl Provider {
    /// `provider-id:model-id` -> provider + model when the alias matches.
    /// Windows paths (`C:\…`) never match, so GGUF roles fall through.
    pub fn split_target<'a>(value: &'a str, providers: &'a [Provider]) -> Option<(&'a Provider, &'a str)> {
        let (alias, model) = value.split_once(':')?;
        let provider = providers.iter().find(|p| p.id.eq_ignore_ascii_case(alias.trim()))?;
        let model = model.trim();
        (!model.is_empty()).then_some((provider, model))
    }
}

/// Model roles; None uses the server's loaded model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct HarnessRoles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
}

/// Per-role server overrides written into the router preset; None inherits.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct RoleServerParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_gpu_layers: Option<i32>,
    /// Detached companion files: sibling stays on disk but is not loaded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_chat_template: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_mmproj: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_draft: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct HarnessRoleParams {
    #[serde(default)]
    pub orchestrator: RoleServerParams,
    #[serde(default)]
    pub worker: RoleServerParams,
}

/// A chat project: a working directory the agent is sandboxed to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct HarnessProject {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub created: u32,
    /// Extra read-only paths outside the project root.
    #[serde(default)]
    pub extra_read: Vec<String>,
}

/// A user-defined language server, spawned over stdio.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct LspServerConfig {
    pub name: String,
    /// Command line; the first token is the program, the rest are argv.
    pub command: String,
    /// File extensions, with or without a leading dot.
    pub extensions: Vec<String>,
    /// LSP languageId sent on didOpen; None uses the extension itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Workspace-root marker files, nearest ancestor wins; empty = project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Vec<String>>,
}

/// Seeded on first load; fully editable afterwards.
pub fn default_lsp_servers() -> Vec<LspServerConfig> {
    let server = |name: &str, command: &str, extensions: &[&str], language: Option<&str>, roots: &[&str]| {
        LspServerConfig {
            name: name.to_string(),
            command: command.to_string(),
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
            language: language.map(str::to_string),
            roots: Some(roots.iter().map(|r| r.to_string()).collect()),
        }
    };
    vec![
        server("rust-analyzer", "rust-analyzer", &["rs"], Some("rust"), &["Cargo.toml"]),
        server(
            "typescript",
            "typescript-language-server --stdio",
            &["ts", "tsx", "mts", "cts"],
            Some("typescript"),
            &["tsconfig.json", "jsconfig.json", "package.json"],
        ),
        server(
            "javascript",
            "typescript-language-server --stdio",
            &["js", "jsx", "mjs", "cjs"],
            Some("javascript"),
            &["tsconfig.json", "jsconfig.json", "package.json"],
        ),
        server(
            "pyright",
            "pyright-langserver --stdio",
            &["py", "pyi"],
            Some("python"),
            &["pyproject.toml", "setup.py", "setup.cfg"],
        ),
        server(
            "clangd",
            "clangd",
            &["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"],
            None,
            &["compile_commands.json", "CMakeLists.txt", "Makefile", "meson.build"],
        ),
        server("gopls", "gopls", &["go"], Some("go"), &["go.mod", "go.work"]),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct AppConfig {
    #[serde(default = "config_version_default")]
    pub version: u32,
    /// First-launch wizard done; `--force-wizard` clears it for this run.
    #[serde(default)]
    pub wizard_completed: bool,
    #[serde(default)]
    pub managed_runtimes: Vec<ManagedRuntime>,
    #[serde(default)]
    pub custom_runtimes: Vec<CustomRuntime>,
    #[serde(default)]
    pub active_runtime: ActiveRuntime,
    #[serde(default)]
    pub model_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub download_dir: Option<PathBuf>,
    #[serde(default)]
    pub favorite_models: Vec<String>,
    #[serde(default)]
    pub selected_model: Option<String>,
    #[serde(default)]
    pub plugin_disabled: Vec<String>,
    /// Turn budget for the orchestrator loop.
    #[serde(default = "default_max_turns")]
    pub harness_max_turns: u32,
    /// Turn budget per subagent run.
    #[serde(default = "default_subagent_turns")]
    pub harness_subagent_max_turns: u32,
    /// Verification-nudge strictness for the agent loop.
    #[serde(default)]
    pub verify_mode: harness::agent::VerifyMode,
    /// Custom system prompt override; None means the built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_system_prompt: Option<String>,
    /// Named system prompt presets (quick swap in Settings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_prompt_presets: Vec<SystemPromptPreset>,
    /// Agent (coding harness) or Roleplay (character cards).
    #[serde(default)]
    pub chat_profile: ChatProfile,
    /// Roleplay: active card, persona, greeting, sampling overrides.
    #[serde(default)]
    pub roleplay: RoleplayConfig,
    /// Housekeeping model for compaction and distillation: a `provider:model`
    /// target or a local role (`orchestrator`/`worker`). None = main model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utility_target: Option<String>,
    /// Language-server diagnostics after edits and the `lsp` query tool.
    #[serde(default = "default_true")]
    pub lsp_enabled: bool,
    /// Single-model or router server mode (Mode tab).
    #[serde(default)]
    pub server_mode: ServerMode,
    /// Agent tool names the harness must not offer; empty enables all.
    #[serde(default)]
    pub agent_tools_disabled: Vec<String>,
    /// Hide `AGENTS.md`/`.agent*` files from every agent tool when set.
    #[serde(default)]
    pub agent_files_hidden: bool,
    /// Extra sensitive-file globs for agent tools (on top of the built-ins).
    #[serde(default)]
    pub sensitive_patterns: Vec<String>,
    /// Exceptions to the sensitive-file policy (globs).
    #[serde(default)]
    pub sensitive_allow: Vec<String>,
    /// Start the local server automatically when a chat run begins.
    #[serde(default)]
    pub server_auto_start: bool,
    /// Stop the local server after this many idle minutes; 0 disables.
    #[serde(default = "default_idle_unload")]
    pub server_idle_unload_minutes: u32,
    /// stdio language servers by name; None seeds the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsp_servers: Option<Vec<LspServerConfig>>,
    /// Which model plans vs executes; a distinct worker needs router mode.
    #[serde(default)]
    pub harness_roles: HarnessRoles,
    /// External OpenAI-compatible providers (managed on the Mode tab).
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// External API mode: the `provider-id:model-id` the harness chats with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_target: Option<String>,
    /// Starred `provider-id:model-id` targets, pinned in the model picker.
    #[serde(default)]
    pub provider_favorites: Vec<String>,
    #[serde(default)]
    pub harness_role_params: HarnessRoleParams,
    #[serde(default)]
    pub harness_projects: Vec<HarnessProject>,
    /// Project the chat view currently has open.
    #[serde(default)]
    pub harness_active_project: Option<String>,
    /// Server process working dir; project fallback for the jail root.
    #[serde(default)]
    pub server_working_dir: Option<String>,
    /// Model path -> preset name; applied on select, stored on start.
    #[serde(default)]
    pub model_presets: HashMap<String, String>,
    /// Last applied preset name; restored at startup when it still exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_preset: Option<String>,
    /// Show the Quick Bench card on the Run page.
    #[serde(default)]
    pub bench_visible: bool,
    /// Delete older builds of the same backend when a new one installs.
    #[serde(default)]
    pub auto_delete_old_runtimes: bool,
    /// HuggingFace sources in preference order (browse filter).
    #[serde(default = "default_preferred_owners")]
    pub preferred_owners: Vec<String>,
    /// Built-in server file tools for every run (subset of known names).
    #[serde(default)]
    pub server_tools: Vec<String>,
    /// MCP servers disabled by name (mcp.json stays complete).
    #[serde(default)]
    pub mcp_disabled: Vec<String>,
    /// Closing hides to the tray instead of quitting.
    #[serde(default)]
    pub close_to_tray: bool,
    /// Check for updates when Settings opens.
    #[serde(default)]
    pub auto_check_updates: bool,
    /// Notification sounds; all default on.
    #[serde(default = "default_true")]
    pub sound_agent: bool,
    #[serde(default = "default_true")]
    pub sound_permissions: bool,
    #[serde(default = "default_true")]
    pub sound_errors: bool,
}

fn config_version_default() -> u32 {
    CONFIG_VERSION
}

fn default_max_turns() -> u32 {
    40
}

fn default_subagent_turns() -> u32 {
    25
}

fn default_true() -> bool {
    true
}

fn default_idle_unload() -> u32 {
    5
}

fn default_preferred_owners() -> Vec<String> {
    crate::models::DEFAULT_PREFERRED_OWNERS.iter().map(|s| s.to_string()).collect()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            wizard_completed: false,
            managed_runtimes: Vec::new(),
            custom_runtimes: Vec::new(),
            active_runtime: ActiveRuntime::None,
            model_dirs: Vec::new(),
            download_dir: None,
            favorite_models: Vec::new(),
            selected_model: None,
            plugin_disabled: Vec::new(),
            harness_max_turns: default_max_turns(),
            harness_subagent_max_turns: default_subagent_turns(),
            verify_mode: Default::default(),
            harness_system_prompt: None,
            system_prompt_presets: Vec::new(),
            chat_profile: ChatProfile::Agent,
            roleplay: RoleplayConfig::default(),
            utility_target: None,
            lsp_enabled: true,
            lsp_servers: Some(default_lsp_servers()),
            server_mode: ServerMode::Single,
            agent_tools_disabled: Vec::new(),
            agent_files_hidden: false,
            sensitive_patterns: Vec::new(),
            sensitive_allow: Vec::new(),
            server_auto_start: false,
            server_idle_unload_minutes: default_idle_unload(),
            harness_roles: Default::default(),
            providers: Vec::new(),
            external_target: None,
            provider_favorites: Vec::new(),
            harness_role_params: Default::default(),
            harness_projects: Vec::new(),
            harness_active_project: None,
            server_working_dir: None,
            model_presets: HashMap::new(),
            last_preset: None,
            bench_visible: false,
            auto_delete_old_runtimes: false,
            preferred_owners: default_preferred_owners(),
            server_tools: Vec::new(),
            mcp_disabled: Vec::new(),
            close_to_tray: false,
            auto_check_updates: false,
            sound_agent: true,
            sound_permissions: true,
            sound_errors: true,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> Result<PathBuf> {
        let data_dir =
            data_dir().ok_or_else(|| anyhow::anyhow!("Cannot find data directory"))?;
        Ok(data_dir.join("werk").join("config.json"))
    }

    pub fn default_models_dir() -> Result<PathBuf> {
        let data_dir =
            data_dir().ok_or_else(|| anyhow::anyhow!("Cannot find data directory"))?;
        Ok(data_dir.join("werk").join("models"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            let default_dir = Self::default_models_dir()?;
            return Ok(Self {
                model_dirs: vec![default_dir.clone()],
                download_dir: Some(default_dir),
                ..Default::default()
            });
        }
        let content = std::fs::read_to_string(&path)?;
        let mut config: Self = serde_json::from_str(&content)
            .context("config.json is corrupt (back it up, then delete it to reset)")?;
        config.migrate();
        // Repair zeroed turn budgets from older builds.
        if config.harness_max_turns == 0 {
            config.harness_max_turns = default_max_turns();
        }
        if config.harness_subagent_max_turns == 0 {
            config.harness_subagent_max_turns = default_subagent_turns();
        }
        if config.model_dirs.is_empty() {
            let default_dir = Self::default_models_dir()?;
            config.model_dirs.push(default_dir.clone());
            if config.download_dir.is_none() {
                config.download_dir = Some(default_dir);
            }
        }
        if config.lsp_servers.is_none() {
            config.lsp_servers = Some(default_lsp_servers());
        }
        Ok(config)
    }

    fn migrate(&mut self) {
        // Missing version keys default to CONFIG_VERSION above; this is the
        // hook for future schema migrations.
        self.version = CONFIG_VERSION;
    }

    /// Atomic save: write tmp + rename, so a crash never halves the file.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn models_dir(&self) -> Result<PathBuf> {
        match &self.download_dir {
            Some(p) => Ok(p.clone()),
            None => Self::default_models_dir(),
        }
    }

    pub fn all_model_dirs(&self) -> Vec<PathBuf> {
        if self.model_dirs.is_empty() {
            Self::default_models_dir().into_iter().collect()
        } else {
            self.model_dirs.clone()
        }
    }
}

/// Platform data dir without pulling dirs as a dependency.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_mode_serializes_external() {
        assert_eq!(serde_json::to_string(&ServerMode::External).unwrap(), "\"external\"");
        let parsed: ServerMode = serde_json::from_str("\"router\"").unwrap();
        assert_eq!(parsed, ServerMode::Router);
    }

    #[test]
    fn provider_targets_split_or_fall_through() {
        let providers = vec![
            Provider {
                id: "deepseek".into(),
                base_url: "https://api.deepseek.com/v1".into(),
                api_key: None,
                models: vec!["deepseek-chat".into()],
                context_length: Some(64_000),
            },
        ];
        let (p, m) = Provider::split_target("deepseek:deepseek-chat", &providers).unwrap();
        assert_eq!(p.id, "deepseek");
        assert_eq!(m, "deepseek-chat");
        // Case-insensitive alias; model keeps its own case and slashes.
        let (_, m) = Provider::split_target("DeepSeek:org/model-1", &providers).unwrap();
        assert_eq!(m, "org/model-1");
        // Windows-style GGUF paths never match an alias.
        assert!(Provider::split_target(r"H:\models\a.gguf", &providers).is_none());
        assert!(Provider::split_target("unknown:model", &providers).is_none());
        assert!(Provider::split_target("deepseek:", &providers).is_none());
    }

    #[test]
    fn default_lsp_servers_cover_common_languages() {
        let servers = default_lsp_servers();
        let covers = |ext: &str| {
            servers
                .iter()
                .any(|s| s.extensions.iter().any(|e| e == ext))
        };
        for ext in ["rs", "ts", "js", "py", "go", "cpp", "c"] {
            assert!(covers(ext), "missing default server for .{ext}");
        }
        assert!(servers
            .iter()
            .all(|s| !s.name.is_empty() && !s.command.is_empty() && !s.extensions.is_empty()));
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let mut config = AppConfig::default();
        config.managed_runtimes.push(ManagedRuntime {
            build: 8000,
            tag_name: "b8000".into(),
            backend_id: "cuda".into(),
            backend_label: "CUDA".into(),
            asset_name: "a.zip".into(),
            dir_name: "b8000-cuda".into(),
            installed_at: 1700000000,
        });
        config.custom_runtimes.push(CustomRuntime {
            label: "mine".into(),
            binary_path: PathBuf::from("/opt/llama/llama-server"),
        });
        config.active_runtime = ActiveRuntime::Managed { build: 8000, backend_id: "cuda".into() };
        config.model_dirs = vec![PathBuf::from("/data/models")];
        config.download_dir = Some(PathBuf::from("/data/models"));
        config.favorite_models = vec!["a".into()];
        config.selected_model = Some("m".into());
        config.plugin_disabled = vec!["p".into()];
        config.harness_max_turns = 60;
        config.verify_mode = harness::agent::VerifyMode::Thorough;
        config.harness_roles = HarnessRoles {
            orchestrator: Some("/m/orch.gguf".into()),
            worker: None,
        };
        config.server_mode = ServerMode::Router;
        config.harness_role_params = HarnessRoleParams {
            orchestrator: RoleServerParams {
                ctx_size: Some(32768),
                n_gpu_layers: None,
                ..Default::default()
            },
            worker: RoleServerParams {
                ctx_size: None,
                n_gpu_layers: Some(99),
                no_mmproj: true,
                ..Default::default()
            },
        };
        config.harness_projects = vec![HarnessProject {
            id: "demo".into(),
            name: "Demo".into(),
            path: "/tmp/demo".into(),
            created: 1700000000,
            extra_read: vec!["/shared".into()],
        }];
        config.harness_active_project = Some("demo".into());
        config.wizard_completed = true;
        config.bench_visible = false;
        config.auto_delete_old_runtimes = true;
        config
            .model_presets
            .insert("/m/qwen.gguf".into(), "fast".into());
        let json = serde_json::to_string_pretty(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.active_runtime, config.active_runtime);
        assert_eq!(restored.managed_runtimes.len(), 1);
        assert_eq!(restored.model_dirs, config.model_dirs);
        assert_eq!(restored.plugin_disabled, vec!["p"]);
        assert_eq!(restored.harness_max_turns, 60);
        assert_eq!(restored.verify_mode, harness::agent::VerifyMode::Thorough);
        assert_eq!(restored.harness_roles.orchestrator.as_deref(), Some("/m/orch.gguf"));
        assert_eq!(restored.server_mode, ServerMode::Router);
        assert_eq!(restored.harness_role_params.orchestrator.ctx_size, Some(32768));
        assert_eq!(restored.harness_role_params.worker.n_gpu_layers, Some(99));
        assert!(restored.harness_role_params.worker.no_mmproj);
        assert_eq!(restored.harness_active_project.as_deref(), Some("demo"));
        assert!(restored.wizard_completed);
        assert!(!restored.bench_visible);
        assert!(restored.auto_delete_old_runtimes);
        assert_eq!(restored.model_presets.get("/m/qwen.gguf").map(String::as_str), Some("fast"));
        assert!(!restored.preferred_owners.is_empty());
        assert!(restored.sound_agent && restored.sound_permissions && restored.sound_errors);
        assert!(restored.server_tools.is_empty());
        assert!(restored.mcp_disabled.is_empty());
        assert!(!restored.close_to_tray);
        assert!(!restored.auto_check_updates);
    }

    #[test]
    fn missing_keys_default_cleanly() {
        let config: AppConfig = serde_json::from_str(r#"{"wizard_completed":true}"#).unwrap();
        assert!(config.wizard_completed);
        assert!(config.model_dirs.is_empty());
        assert_eq!(config.active_runtime, ActiveRuntime::None);
        assert_eq!(config.version, CONFIG_VERSION);
    }
}
