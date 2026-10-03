//! Embedded project terminal: one shell command at a time, run in the
//! project folder with streamed output. This is the user's own shell — the
//! agent's jail and approvals do not apply.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{Emitter, Manager};

pub struct TerminalRuntime {
    child: Mutex<Option<Child>>,
    cwd: Mutex<Option<PathBuf>>,
    seq: AtomicU64,
}

impl TerminalRuntime {
    pub fn new() -> Self {
        Self {
            child: Mutex::new(None),
            cwd: Mutex::new(None),
            seq: AtomicU64::new(0),
        }
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut c = crate::hidden::command("powershell");
        c.args(["-NoLogo", "-NoProfile", "-Command", command]);
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = crate::hidden::command("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// Kill the command and anything it spawned (children hold the pipes).
fn kill_tree(child: &mut Child) {
    let pid = child.id();
    #[cfg(target_os = "windows")]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn project_cwd(state: &crate::AppState) -> Result<PathBuf, String> {
    crate::chat::project_root(state).map_err(|e| e.to_string())
}

/// Current terminal working directory; `reset` re-derives it from the project.
#[tauri::command]
#[specta::specta]
pub async fn terminal_cwd(
    reset: bool,
    state: tauri::State<'_, crate::AppState>,
) -> Result<String, String> {
    if !reset {
        if let Some(cwd) = state.terminal.cwd.lock().unwrap().clone() {
            return Ok(cwd.to_string_lossy().to_string());
        }
    }
    let root = project_cwd(&state)?;
    *state.terminal.cwd.lock().unwrap() = Some(root.clone());
    Ok(root.to_string_lossy().to_string())
}

/// Run one command in the project folder; output streams over
/// `terminal_output` / `terminal_done`.
#[tauri::command]
#[specta::specta]
pub async fn terminal_exec(
    app: tauri::AppHandle,
    command: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let command = command.trim().to_string();
    if command.is_empty() {
        return Ok(());
    }
    // One command at a time.
    {
        let mut slot = state.terminal.child.lock().unwrap();
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => *slot = None,
                Ok(None) => return Err("A command is still running".to_string()),
            }
        }
    }
    let seq = state.terminal.seq.fetch_add(1, Ordering::SeqCst) + 1;
    let emit = |line: &str, err: bool| {
        let _ = app.emit(
            "terminal_output",
            serde_json::json!({ "seq": seq, "line": line, "err": err }),
        );
    };
    // `cd` is tracked in the runtime so the next command starts there.
    if let Some(target) = command.strip_prefix("cd ") {
        let target = target.trim().trim_matches(['"', '\'']);
        let base = {
            let tracked = state.terminal.cwd.lock().unwrap().clone();
            match tracked {
                Some(cwd) => cwd,
                None => project_cwd(&state)?,
            }
        };
        let path = if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            base.join(target)
        };
        let path = path
            .canonicalize()
            .map_err(|_| format!("No such folder: {target}"))?;
        if !path.is_dir() {
            return Err(format!("Not a folder: {target}"));
        }
        *state.terminal.cwd.lock().unwrap() = Some(path.clone());
        emit(&path.to_string_lossy(), false);
        let _ = app.emit("terminal_done", serde_json::json!({ "seq": seq, "code": 0 }));
        return Ok(());
    }
    let cwd = {
        let tracked = state.terminal.cwd.lock().unwrap().clone();
        match tracked {
            Some(cwd) => cwd,
            None => project_cwd(&state)?,
        }
    };
    let mut cmd = shell_command(&command);
    cmd.current_dir(&cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let stderr = child.stderr.take().ok_or("no stderr pipe")?;
    *state.terminal.child.lock().unwrap() = Some(child);

    // Reader threads feed lines over a channel; a task forwards them and
    // reports the exit code, polling the child so Kill stays reachable.
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let tx_out = tx.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx_out.send((false, line));
        }
    });
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = tx.send((true, line));
        }
    });
    let app_done = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut code: Option<i32> = None;
        loop {
            while let Ok((err, line)) = rx.try_recv() {
                let _ = app_done.emit(
                    "terminal_output",
                    serde_json::json!({ "seq": seq, "line": line, "err": err }),
                );
            }
            let exited = {
                let Some(state) = app_done.try_state::<crate::AppState>() else {
                    break;
                };
                let mut slot = state.terminal.child.lock().unwrap();
                match slot.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => {
                            code = status.code();
                            *slot = None;
                            true
                        }
                        Ok(None) => false,
                        Err(_) => {
                            *slot = None;
                            true
                        }
                    },
                    // Killed from the UI.
                    None => true,
                }
            };
            if exited {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                while let Ok((err, line)) = rx.try_recv() {
                    let _ = app_done.emit(
                        "terminal_output",
                        serde_json::json!({ "seq": seq, "line": line, "err": err }),
                    );
                }
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        let _ = app_done.emit("terminal_done", serde_json::json!({ "seq": seq, "code": code }));
    });
    Ok(())
}

/// Stop the running command (and its children).
#[tauri::command]
#[specta::specta]
pub async fn terminal_kill(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    let child = state.terminal.child.lock().unwrap().take();
    if let Some(mut child) = child {
        kill_tree(&mut child);
    }
    Ok(())
}
