//! Werk backend: drivers plus a thin command layer. Commands validate,
//! call plain functions, and map errors — no business logic lives here.

pub mod assistant;
pub mod bench;
pub mod browser;
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
pub mod run_context;
pub mod recommended;
pub mod roleplay;
pub mod runtime;
pub mod scheduler;
pub mod server;
pub mod system;
pub mod terminal;
pub mod worktree;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(debug_assertions)]
use specta_typescript::Typescript;
use tauri_specta::{collect_commands, Builder};

/// Show and focus the main window (tray, single instance).
pub fn show_main(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// The overlay window, built on demand. While the overlay is off no window
/// (and no WebView) exists, so it costs nothing; enabling it rebuilds this.
fn ensure_overlay(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
    if let Some(win) = app.get_webview_window("overlay") {
        return Ok(win);
    }
    let built = WebviewWindowBuilder::new(
        app,
        "overlay",
        WebviewUrl::App("index.html?overlay=1".into()),
    )
    .title("Werk Assistant")
    .inner_size(84.0, 84.0)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .focused(false)
    .build();
    let win = match built {
        Ok(win) => win,
        // A concurrent caller may have built it first.
        Err(e) => return app.get_webview_window("overlay").ok_or(e),
    };
    // Anchor bottom-right of the main window's work area before the first
    // show, so it never flashes at the default position.
    #[cfg(target_os = "windows")]
    if let Some(area) = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .and_then(|h| monitor_work_area(windows::Win32::Foundation::HWND(h.0 as _)))
    {
        let scale = win.scale_factor().unwrap_or(1.0);
        let side = (84.0 * scale).round() as i32;
        let margin = (16.0 * scale).round() as i32;
        let raise = (16.0 * scale).round() as i32;
        let _ = win.set_size(tauri::PhysicalSize::new(side as u32, side as u32));
        let _ = win.set_position(tauri::PhysicalPosition::new(
            area.right - side - margin,
            area.bottom - side - margin - raise,
        ));
    }
    Ok(win)
}

/// Show and focus the assistant overlay, asking it to open its input.
pub fn show_overlay(app: &tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    if let Some(state) = app.try_state::<AppState>() {
        if !state.config.lock().unwrap().assistant.overlay_enabled {
            return;
        }
    }
    if let Ok(win) = ensure_overlay(app) {
        // Re-assert topmost: the taskbar is topmost too and can win z-order.
        let _ = win.set_always_on_top(true);
        let _ = win.show();
        let _ = win.set_focus();
        let _ = app.emit_to("overlay", "assistant_focus", ());
        // A freshly built window may not have its listeners yet.
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = handle.emit_to("overlay", "assistant_focus", ());
        });
    }
}

/// Physical work-area rect (excludes the taskbar) of the overlay's monitor.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Work area (excludes the taskbar) of the monitor nearest `hwnd`.
#[cfg(target_os = "windows")]
fn monitor_work_area(hwnd: windows::Win32::Foundation::HWND) -> Option<WorkArea> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let r = info.rcWork;
        Some(WorkArea {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        })
    }
}

/// The overlay's work area; None outside Windows (the frontend falls back to
/// the full monitor rect).
#[tauri::command]
#[specta::specta]
fn overlay_work_area(app: tauri::AppHandle) -> Option<WorkArea> {
    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;
        use windows::Win32::Foundation::HWND;
        let win = app.get_webview_window("overlay")?;
        monitor_work_area(HWND(win.hwnd().ok()?.0 as _))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        None
    }
}

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
    /// Roleplay/Talk transcript and run state; separate from `harness` so the
    /// two chats never mix.
    pub talk: Arc<chat::HarnessRuntime>,
    /// Assistant transcript and run state; same separation as `talk`.
    pub assistant: Arc<chat::HarnessRuntime>,
    /// Reminder scheduler paused (tray toggle); notifications and proactive
    /// turns stop, the transcript stays.
    pub assistant_paused: AtomicBool,
    /// Language servers for diagnostics and the `lsp` tool.
    pub lsp: Arc<harness::lsp::LspManager>,
    /// Harness-side MCP servers (spawned lazily, reused across runs).
    pub mcp_agent: mcp::McpAgent,
    /// Embedded project terminal (user-driven).
    pub terminal: terminal::TerminalRuntime,
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

/// Show or fully tear down the assistant overlay (the frontend owns profile
/// awareness). Destroying frees the WebView; it is rebuilt on demand.
#[tauri::command]
#[specta::specta]
fn set_overlay_visible(visible: bool, app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if visible {
        let win = ensure_overlay(&app).map_err(|e| e.to_string())?;
        win.show().map_err(|e| e.to_string())?;
    } else if let Some(win) = app.get_webview_window("overlay") {
        win.destroy().map_err(|e| e.to_string())?;
    }
    Ok(())
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
        set_overlay_visible,
        overlay_work_area,
        chat::harness_agent_send,
        chat::harness_agent_abort,
        chat::harness_agent_steer,
        chat::harness_agent_reset,
        chat::roleplay_start_chat,
        chat::talk_send,
        chat::talk_abort,
        chat::talk_history,
        chat::talk_rewind,
        chat::talk_compact,
        chat::talk_distill,
        chat::talk_context_stats,
        chat::talk_question_answer,
        chat::assistant_send,
        chat::assistant_abort,
        chat::assistant_history,
        chat::assistant_rewind,
        chat::assistant_compact,
        chat::assistant_distill,
        chat::assistant_context_stats,
        chat::assistant_question_answer,
        chat::assistant_reset,
        chat::assistant_decide,
        chat::roleplay_list_discussions,
        chat::roleplay_load_discussion,
        chat::roleplay_delete_discussion,
        chat::roleplay_export_discussion,
        chat::roleplay_import_discussion,
        chat::harness_todos_clear,
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
        chat::set_role_draft,
        chat::set_role_attachment,
        chat::set_max_turns,
        chat::set_verify_mode,
        chat::set_system_prompt,
        chat::set_system_prompt_presets,
        assistant::set_assistant_config,
        assistant::set_assistant_behavior,
        assistant::assistant_system_prompt_default,
        assistant::set_assistant_access,
        assistant::set_assistant_fs,
        assistant::assistant_temp_dir,
        assistant::assistant_set_avatar,
        assistant::assistant_avatar,
        scheduler::assistant_reminders_list,
        scheduler::assistant_reminder_add,
        scheduler::assistant_reminder_complete,
        scheduler::assistant_reminder_remove,
        roleplay::roleplay_list_cards,
        roleplay::roleplay_import_card,
        roleplay::roleplay_get_card,
        roleplay::roleplay_set_card_prompt,
        roleplay::roleplay_update_card,
        roleplay::roleplay_export_card,
        roleplay::roleplay_delete_card,
        roleplay::roleplay_card_avatar,
        roleplay::set_chat_profile,
        roleplay::set_webui_enabled,
        roleplay::set_roleplay_config,
        roleplay::roleplay_set_user_avatar,
        roleplay::roleplay_user_avatar,
        roleplay::roleplay_system_prompt_default,
        chat::set_utility_target,
        chat::set_lsp_enabled,
        chat::set_agent_files_hidden,
        chat::set_sensitive_shielding,
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
        chat::scaffold_plugin,

        chat::skills_list,
        chat::harness_memory_get,
        chat::harness_memory_set,
        chat::harness_memory_entries,
        chat::harness_memory_save_entries,
        chat::harness_memory_ensure,
        chat::skills_ensure_dir,
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
        commands::project_rename_entry,
        commands::project_delete_entry,
        commands::stop_server,
        commands::set_server_mode,
        commands::set_providers,
        commands::set_external_target,
        commands::set_provider_favorites,
        commands::test_provider,
        commands::start_server,
        commands::ensure_server,
        commands::set_server_lifecycle,
        terminal::terminal_cwd,
        terminal::terminal_exec,
        terminal::terminal_kill,
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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        show_overlay(app);
                    }
                })
                .build(),
        )
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&["overlay"])
                .build(),
        )
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
            // Load persisted approval grants once (compacts stale files).
            {
                let state = app.state::<AppState>();
                chat::load_permission_grants(state.inner());
            }
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
            use tauri::menu::{CheckMenuItem, Menu, MenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
            if let Some(icon) = app.default_window_icon().cloned() {
                let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
                let assistant =
                    MenuItem::with_id(app, "assistant", "Assistant", true, None::<&str>)?;
                let pause = CheckMenuItem::with_id(
                    app,
                    "pause",
                    "Pause reminders",
                    true,
                    false,
                    None::<&str>,
                )?;
                let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show, &assistant, &pause, &quit])?;
                let pause_handle = pause.clone();
                TrayIconBuilder::with_id("main")
                    .icon(icon)
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(move |app, event| match event.id.as_ref() {
                        "show" => show_main(app),
                        "assistant" => {
                            use tauri::Emitter;
                            show_main(app);
                            let _ = app.emit("assistant_open", ());
                        }
                        "pause" => {
                            use tauri::Manager;
                            if let Some(state) = app.try_state::<AppState>() {
                                let now = !state.assistant_paused.load(Ordering::SeqCst);
                                state.assistant_paused.store(now, Ordering::SeqCst);
                                let _ = pause_handle.set_checked(now);
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
                            show_main(tray.app_handle());
                        }
                    })
                    .build(app)?;
            }
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_visible(tray_on_startup);
            }
            // Assistant always-on: hotkey, autostart sync, reminder tick.
            {
                use tauri_plugin_autostart::ManagerExt;
                use tauri_plugin_global_shortcut::GlobalShortcutExt;
                let assistant = app.state::<AppState>().config.lock().unwrap().assistant.clone();
                if assistant.overlay_enabled && !assistant.hotkey.trim().is_empty() {
                    let _ = app.global_shortcut().register(assistant.hotkey.as_str());
                }
                let _ = if assistant.autostart {
                    app.autolaunch().enable()
                } else {
                    app.autolaunch().disable()
                };
            }
            scheduler::spawn(app.handle().clone());
            // Idle unload: stop the local server after the configured idle
            // window, unless the Web UI is enabled (it may be in use outside
            // the chat).
            let idle_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tauri::{Emitter, Manager};
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    let Some(state) = idle_handle.try_state::<AppState>() else {
                        continue;
                    };
                    let minutes = state.config.lock().unwrap().server_idle_unload_minutes;
                    if minutes == 0
                        || state.harness.running.load(Ordering::SeqCst)
                        || state.talk.running.load(Ordering::SeqCst)
                        || state.assistant.running.load(Ordering::SeqCst)
                    {
                        continue;
                    }
                    {
                        let s = state.server.lock().unwrap();
                        if !matches!(
                            s.status,
                            server::ServerStatus::Running { ready: true, .. }
                        ) {
                            continue;
                        }
                        let webui = s
                            .config
                            .as_ref()
                            .map(|c| !c.extra_params.contains_key("no-webui"))
                            .unwrap_or(false);
                        if webui {
                            continue;
                        }
                    }
                    let idle = {
                        let agent = state.harness.last_activity.lock().unwrap().elapsed();
                        let talk = state.talk.last_activity.lock().unwrap().elapsed();
                        let assistant = state.assistant.last_activity.lock().unwrap().elapsed();
                        agent.min(talk).min(assistant)
                    };
                    if idle < std::time::Duration::from_secs(minutes as u64 * 60) {
                        continue;
                    }
                    let _ = idle_handle.emit(
                        "server_log",
                        format!("Stopping the server after {minutes} idle minutes."),
                    );
                    let _ = server::stop_server(&state.server).await;
                }
            });
            Ok(())
        })
        .on_window_event(|win, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                use tauri::Manager;
                // The overlay is rebuilt on demand: closing it destroys the
                // WebView instead of keeping a hidden one alive.
                if win.label() == "overlay" {
                    api.prevent_close();
                    let _ = win.destroy();
                    return;
                }
                let tray = win
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|s| s.config.lock().unwrap().close_to_tray)
                    .unwrap_or(false);
                if tray {
                    api.prevent_close();
                    let _ = win.hide();
                } else {
                    // The hidden overlay would otherwise keep the app alive.
                    win.app_handle().exit(0);
                }
            }
        })
        .manage(AppState {
            config: Mutex::new(config),
            server: server::new_server_state(),
            http_client,
            dl_client,
            harness: Arc::new(chat::HarnessRuntime::new()),
            talk: Arc::new(chat::HarnessRuntime::new()),
            assistant: Arc::new(chat::HarnessRuntime::new()),
            assistant_paused: AtomicBool::new(false),
            lsp,
            mcp_agent: mcp::McpAgent::default(),
            terminal: terminal::TerminalRuntime::new(),
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
            talk: Arc::new(chat::HarnessRuntime::new()),
            assistant: Arc::new(chat::HarnessRuntime::new()),
            assistant_paused: AtomicBool::new(false),
            lsp: Arc::new(harness::lsp::LspManager::new()),
            mcp_agent: mcp::McpAgent::default(),
            terminal: terminal::TerminalRuntime::new(),
            downloads: Mutex::new(HashMap::new()),
        };
        assert!(state.harness.history.lock().unwrap().is_empty());
    }
}
