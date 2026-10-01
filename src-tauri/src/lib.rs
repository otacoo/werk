//! Werk backend: drivers plus a thin command layer. Commands validate,
//! call plain functions, and map errors — no business logic lives here.

pub mod bench;
pub mod chat;
pub mod commands;
pub mod config;
pub mod download;
pub mod estimate;
pub mod hardware;
mod hidden;
#[cfg(target_os = "windows")]
mod caption_hit_test;
pub mod mcp;
pub mod models;
pub mod presets;
pub mod recommended;
pub mod runtime;
pub mod server;
pub mod worktree;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[cfg(debug_assertions)]
use specta_typescript::Typescript;
use tauri_specta::{collect_commands, Builder};

/// Config rows -> spawnable specs; unusable rows are skipped.
pub fn lsp_specs(config: &config::AppConfig) -> Vec<harness::lsp::servers::ServerSpec> {
    config
        .lsp_servers
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|server| {
            harness::lsp::servers::ServerSpec::build(
                &server.name,
                &server.command,
                &server.extensions,
                server.language.as_deref(),
                server.roots.as_deref().unwrap_or(&[]),
            )
        })
        .collect()
}

/// Shared app state: config, server, HTTP, and the harness runtime.
pub struct AppState {
    pub config: Mutex<config::AppConfig>,
    pub server: server::SharedServerState,
    pub http_client: reqwest::Client,
    /// File downloads: no total timeout (bodies outlive any deadline).
    pub dl_client: reqwest::Client,
    pub harness: Arc<chat::HarnessRuntime>,
    pub harness_abort: Arc<AtomicBool>,
    /// Language servers for diagnostics and the `lsp` tool.
    pub lsp: Arc<harness::lsp::LspManager>,
    /// Harness-side MCP servers (spawned lazily, reused across runs).
    pub mcp_agent: mcp::McpAgent,
    /// In-flight downloads by id (`runtime:<asset>`, filenames); set to stop.
    pub downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

/// Proves typed IPC end to end.
#[tauri::command]
#[specta::specta]
fn ping() -> String {
    harness::ping().to_string()
}

/// Crate version for the About surface.
#[tauri::command]
#[specta::specta]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
#[specta::specta]
fn get_config(state: tauri::State<'_, AppState>) -> Result<config::AppConfig, String> {
    Ok(state.config.lock().unwrap().clone())
}

#[cfg(debug_assertions)]
fn export_bindings<R: tauri::Runtime>(builder: &Builder<R>) {
    // Absolute: relative paths resolve against the process CWD, which
    // differs between `cargo run`, the dev server, and the built app.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/bindings.ts");
    builder
        .export(Typescript::default(), path)
        .expect("export typescript bindings");
}

/// Canonical command list, shared by the app and the export bin so the
/// generated bindings can never drift from the handler.
pub fn bindings_builder() -> Builder<tauri::Wry> {
    Builder::new().commands(collect_commands![
        ping,
        app_version,
        get_config,
        chat::harness_agent_send,
        chat::harness_agent_abort,
        chat::harness_agent_steer,
        chat::harness_agent_reset,
        chat::harness_distill,
        chat::harness_agent_history,
        chat::harness_agent_decide,
        chat::harness_question_answer,
        chat::get_harness_system_prompt_default,
        chat::harness_sessions_list,
        chat::harness_session_load,
        chat::harness_session_delete,
        chat::harness_session_undo,
        chat::harness_session_rename,
        chat::harness_session_export,
        chat::harness_project_add,
        chat::harness_project_remove,
        chat::harness_project_rename,
        chat::set_harness_project_extra_read,
        chat::harness_git_is_repo,
        chat::harness_worktree_list,
        chat::harness_worktree_add,
        chat::harness_worktree_remove,
        chat::harness_project_set_active,
        chat::set_harness_roles,
        chat::set_role_params,
        chat::set_role_attachment,
        chat::set_max_turns,
        chat::set_verify_mode,
        chat::set_system_prompt,
        chat::set_system_prompt_presets,
        chat::set_utility_target,
        chat::set_lsp_enabled,
        chat::set_agent_tool_enabled,
        chat::set_lsp_servers,
        chat::reset_lsp_servers,
        chat::set_sound_agent,
        chat::set_sound_permissions,
        chat::set_sound_errors,
        chat::harness_agent_rewind,
        chat::harness_agent_compact,
        chat::harness_read_attachment,
        chat::harness_agent_capabilities,
        chat::harness_reasoning_options,
        chat::harness_context_stats,
        chat::tools_list,
        chat::plugin_set_enabled,
        chat::skills_list,
        chat::harness_memory_get,
        chat::harness_memory_set,
        chat::get_platform_style,
        commands::set_wizard_completed,
        commands::get_system_info,
        commands::check_release,
        commands::get_server_status,
        commands::get_server_logs,
        commands::get_server_tools,
        commands::get_server_info,
        commands::set_tools,
        commands::list_mcp_servers,
        commands::save_mcp_servers,
        commands::get_mcp_agent_tools,
        commands::project_file_tree,
        commands::stop_server,
        commands::set_server_mode,
        commands::set_providers,
        commands::set_external_target,
        commands::set_provider_favorites,
        commands::test_provider,
        commands::start_server,
        commands::preview_server_args,
        commands::list_installed_models,
        commands::delete_model,
        commands::set_selected_model,
        commands::toggle_favorite_model,
        commands::get_model_dirs,
        commands::add_model_dir,
        commands::remove_model_dir,
        commands::set_download_dir,
        commands::get_hf_repo_files,
        commands::search_hf_models,
        commands::get_known_owners,
        commands::set_preferred_owners,
        commands::validate_hf_owner,
        commands::download_model,
        commands::cancel_download,
        commands::pause_download,
        commands::discard_download,
        commands::get_runtime_info,
        commands::download_runtime_latest,
        commands::download_runtime_asset,
        commands::scan_custom_binaries,
        commands::recommended_models,
        commands::set_active_runtime,
        commands::delete_managed_runtime,
        commands::set_auto_delete_runtimes,
        commands::add_custom_runtime,
        commands::remove_custom_runtime,
        commands::list_presets,
        commands::reset_default_preset,
        commands::load_preset,
        commands::save_preset,
        commands::delete_preset,
        commands::get_model_preset,
        commands::set_model_preset,
        commands::set_last_preset,
        commands::set_bench_visible,
        commands::set_close_to_tray,
        commands::set_auto_check_updates,
        commands::run_bench,
        commands::bench_history,
        commands::clear_bench_history,
        commands::estimate_memory,
        commands::suggest_server_config,
        commands::suggest_model_config,
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut config = match config::AppConfig::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: failed to load config: {e}");
            config::AppConfig::default()
        }
    };
    if std::env::args().any(|a| a == "--force-wizard") {
        config.wizard_completed = false;
    }
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");
    // Downloads outlive any total deadline; user cancel stops them instead.
    let dl_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build download client");
    let builder = bindings_builder();
    #[cfg(debug_assertions)]
    export_bindings(&builder);
    // The tray toggle owns the icon: start hidden when it is off.
    let tray_on_startup = config.close_to_tray;
    let lsp = Arc::new(harness::lsp::LspManager::new());
    lsp.set_specs(lsp_specs(&config));
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Second launch focuses the existing window instead of a copy.
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .setup(move |app| {
            use tauri::Manager;
            // Caption buttons must win over the resize border at the corner.
            #[cfg(target_os = "windows")]
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(hwnd) = window.hwnd() {
                    caption_hit_test::install(hwnd.0 as isize);
                }
            }
            // Paint the window surface with the system theme so the reveal (and
            // any pre-paint frame) never flashes white.
            if let Some(window) = app.get_webview_window("main") {
                let theme_color = |theme: &tauri::Theme| match theme {
                    tauri::Theme::Light => tauri::window::Color(249, 250, 251, 255),
                    _ => tauri::window::Color(20, 20, 20, 255),
                };
                if let Ok(theme) = window.theme() {
                    let _ = window.set_background_color(Some(theme_color(&theme)));
                }
                let watched = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::ThemeChanged(theme) = event {
                        let _ = watched.set_background_color(Some(theme_color(theme)));
                    }
                });
            }
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
            if let Some(icon) = app.default_window_icon().cloned() {
                let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show, &quit])?;
                TrayIconBuilder::with_id("main")
                    .icon(icon)
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => {
                            use tauri::Manager;
                            if let Some(win) = app.get_webview_window("main") {
                                let _ = win.unminimize();
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            use tauri::Manager;
                            let app = tray.app_handle();
                            if let Some(win) = app.get_webview_window("main") {
                                let _ = win.unminimize();
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    })
                    .build(app)?;
            }
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_visible(tray_on_startup);
            }
            Ok(())
        })
        .on_window_event(|win, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                use tauri::Manager;
                let tray = win
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|s| s.config.lock().unwrap().close_to_tray)
                    .unwrap_or(false);
                if tray {
                    api.prevent_close();
                    let _ = win.hide();
                }
            }
        })
        .manage(AppState {
            config: Mutex::new(config),
            server: server::new_server_state(),
            http_client,
            dl_client,
            harness: Arc::new(chat::HarnessRuntime::new()),
            harness_abort: Arc::new(AtomicBool::new(false)),
            lsp,
            mcp_agent: mcp::McpAgent::default(),
            downloads: Mutex::new(HashMap::new()),
        })
        .invoke_handler(builder.invoke_handler())
        .build(tauri::generate_context!())
        .expect("error while building werk application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                if let Some(state) = app.try_state::<AppState>() {
                    state.lsp.shutdown_all();
                    server::kill_server_sync(&state.server);
                    state.mcp_agent.shutdown();
                }
                bench::kill_running();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_replies() {
        assert_eq!(ping(), "pong");
    }

    #[test]
    fn app_state_builds_from_default_config() {
        let state = AppState {
            config: Mutex::new(config::AppConfig::default()),
            server: server::new_server_state(),
            http_client: reqwest::Client::new(),
            dl_client: reqwest::Client::new(),
            harness: Arc::new(chat::HarnessRuntime::new()),
            harness_abort: Arc::new(AtomicBool::new(false)),
            lsp: Arc::new(harness::lsp::LspManager::new()),
            mcp_agent: mcp::McpAgent::default(),
            downloads: Mutex::new(HashMap::new()),
        };
        assert!(state.harness.history.lock().unwrap().is_empty());
    }
}
