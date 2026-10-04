//! Assistant browser control: Chrome/Edge over CDP (chromiumoxide) and
//! Firefox over native WebDriver BiDi. Both attach to a browser launched with
//! a debug port; Firefox uses an isolated profile unless the user opts into
//! their own profile.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use harness::permissions::ApprovalKey;
use harness::tools::Tool;

const CHROMIUM_PORT: u16 = 9224;
const FIREFOX_PORT: u16 = 9225;

pub struct BrowserTool {
    /// Drive the user's own Firefox profile instead of an isolated one.
    user_profile: bool,
}

impl BrowserTool {
    pub fn new(user_profile: bool) -> Self {
        Self { user_profile }
    }
}

impl Tool for BrowserTool {
    fn name(&self) -> String {
        "browser".to_string()
    }

    fn description(&self) -> String {
        let firefox = if self.user_profile {
            "your own Firefox profile (quit Firefox before it is first launched)"
        } else {
            "an isolated profile"
        };
        format!(
            "Control a web browser. `open` (url), `read` (page text), `eval` (script), `click` \
             (selector), `type` (selector + text), `screenshot`, `tabs`. Pick `browser`: chrome, \
             edge, or firefox. Firefox runs with {firefox}; Chrome/Edge always use an isolated \
             profile. The browser stays open; screenshots land in the temp workspace."
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["open", "read", "eval", "click", "type", "screenshot", "tabs"] },
                "browser": { "type": "string", "enum": ["chrome", "edge", "firefox"], "description": "Default chrome." },
                "url": { "type": "string", "description": "For open." },
                "selector": { "type": "string", "description": "CSS selector for click/type." },
                "text": { "type": "string", "description": "Text to type." },
                "script": { "type": "string", "description": "JavaScript for eval." }
            },
            "required": ["action"]
        })
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("'action' is required"))?
            .to_string();
        let browser = args
            .get("browser")
            .and_then(Value::as_str)
            .unwrap_or("chrome")
            .to_lowercase();
        let args = args.clone();
        let user_profile = self.user_profile;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow!("Cannot start the browser runtime: {e}"))?;
        runtime.block_on(async move {
            match browser.as_str() {
                "firefox" => firefox(&action, &args, user_profile).await,
                "chrome" | "edge" => chromium(&browser, &action, &args).await,
                other => bail!("Unknown browser '{other}' (chrome, edge, firefox)"),
            }
        })
    }

    fn result_images(&self, _args: &Value, output: &str) -> Vec<(String, Vec<u8>)> {
        let Some(path) = output.strip_prefix("Saved screenshot to ") else {
            return Vec::new();
        };
        harness::tools::image_attachment(std::path::Path::new(path.trim()))
    }
}

fn req<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("'{name}' is required"))
}

fn cap(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}\n[…truncated]")
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

fn screenshot_path() -> Result<PathBuf> {
    let dir = crate::system::temp_workspace().join("browser");
    std::fs::create_dir_all(&dir)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(dir.join(format!("shot-{millis}.png")))
}

fn port_open(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    addr.parse()
        .ok()
        .and_then(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_millis(300)).ok())
        .is_some()
}

fn wait_for_port(port: u16, label: &str) -> Result<()> {
    for _ in 0..60 {
        if port_open(port) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("{label} did not open its debug port")
}

// ── Chrome / Edge (CDP) ───────────────────────────────────────────────────

fn chromium_candidates(kind: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let pf = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let pf86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    if kind == "edge" {
        if let Some(p) = &pf86 {
            out.push(p.join("Microsoft/Edge/Application/msedge.exe"));
        }
        if let Some(p) = &pf {
            out.push(p.join("Microsoft/Edge/Application/msedge.exe"));
        }
        out.push(PathBuf::from("/usr/bin/microsoft-edge"));
        out.push(PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"));
    } else {
        if let Some(p) = &pf {
            out.push(p.join("Google/Chrome/Application/chrome.exe"));
        }
        if let Some(p) = &pf86 {
            out.push(p.join("Google/Chrome/Application/chrome.exe"));
        }
        if let Some(p) = &local {
            out.push(p.join("Google/Chrome/Application/chrome.exe"));
        }
        out.push(PathBuf::from("/usr/bin/google-chrome"));
        out.push(PathBuf::from("/usr/bin/chromium"));
        out.push(PathBuf::from("/usr/bin/chromium-browser"));
        out.push(PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
    }
    out
}

fn ensure_chromium(kind: &str) -> Result<()> {
    if port_open(CHROMIUM_PORT) {
        return Ok(());
    }
    let exe = chromium_candidates(kind)
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            let label = if kind == "edge" { "Edge" } else { "Chrome" };
            anyhow!("{label} is not installed — install it or use browser: firefox")
        })?;
    let profile = crate::system::temp_workspace()
        .join("browser")
        .join(format!("{kind}-profile"));
    std::fs::create_dir_all(&profile)?;
    Command::new(&exe)
        .arg(format!("--remote-debugging-port={CHROMIUM_PORT}"))
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Cannot launch {}: {e}", exe.display()))?;
    wait_for_port(CHROMIUM_PORT, kind)
}

async fn chromium(kind: &str, action: &str, args: &Value) -> Result<String> {
    use chromiumoxide::browser::Browser;
    use futures::StreamExt;

    ensure_chromium(kind)?;
    let (mut browser, mut handler) = Browser::connect(format!("http://127.0.0.1:{CHROMIUM_PORT}"))
        .await
        .map_err(|e| anyhow!("Cannot attach to {kind}: {e}"))?;
    let pump = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let result = chromium_action(&mut browser, action, args).await;
    drop(browser);
    pump.abort();
    result
}

async fn chromium_page(
    browser: &mut chromiumoxide::browser::Browser,
) -> Result<chromiumoxide::page::Page> {
    let _ = browser.fetch_targets().await;
    let pages = browser
        .pages()
        .await
        .map_err(|e| anyhow!("Cannot list tabs: {e}"))?;
    if let Some(page) = pages.into_iter().last() {
        return Ok(page);
    }
    browser
        .new_page("about:blank")
        .await
        .map_err(|e| anyhow!("Cannot open a tab: {e}"))
}

async fn chromium_action(
    browser: &mut chromiumoxide::browser::Browser,
    action: &str,
    args: &Value,
) -> Result<String> {
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use chromiumoxide::page::ScreenshotParams;

    let page = chromium_page(browser).await?;
    let err = |e: chromiumoxide::error::CdpError| anyhow!(e.to_string());
    match action {
        "open" => {
            let url = req(args, "url")?;
            page.goto(url).await.map_err(err)?;
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            Ok(format!("Opened {url} ({title})."))
        }
        "read" => {
            let text: String = page
                .evaluate("document.body ? document.body.innerText : ''")
                .await
                .map_err(err)?
                .into_value()
                .unwrap_or_default();
            Ok(cap(text, 20_000))
        }
        "eval" => {
            let script = req(args, "script")?;
            let value: Value = page
                .evaluate(script)
                .await
                .map_err(err)?
                .into_value()
                .unwrap_or(Value::Null);
            Ok(cap(
                serde_json::to_string_pretty(&value).unwrap_or_default(),
                20_000,
            ))
        }
        "click" => {
            let selector = req(args, "selector")?;
            let script = format!(
                "(() => {{ const el = document.querySelector({sel}); if (!el) return false; \
                 el.scrollIntoView({{block:'center'}}); el.click(); return true; }})()",
                sel = json_string(selector)
            );
            let clicked: bool = page
                .evaluate(script)
                .await
                .map_err(err)?
                .into_value()
                .unwrap_or(false);
            if !clicked {
                bail!("No element matches '{selector}'");
            }
            Ok(format!("Clicked {selector}."))
        }
        "type" => {
            let selector = req(args, "selector")?;
            let text = req(args, "text")?;
            let script = format!(
                "(() => {{ const el = document.querySelector({sel}); if (!el) return false; \
                 el.focus(); el.value = {text}; \
                 el.dispatchEvent(new Event('input', {{bubbles:true}})); \
                 el.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()",
                sel = json_string(selector),
                text = json_string(text)
            );
            let typed: bool = page
                .evaluate(script)
                .await
                .map_err(err)?
                .into_value()
                .unwrap_or(false);
            if !typed {
                bail!("No element matches '{selector}'");
            }
            Ok(format!("Typed {} characters into {selector}.", text.chars().count()))
        }
        "screenshot" => {
            let bytes = page
                .screenshot(
                    ScreenshotParams::builder()
                        .format(CaptureScreenshotFormat::Png)
                        .build(),
                )
                .await
                .map_err(err)?;
            let path = screenshot_path()?;
            std::fs::write(&path, bytes)?;
            Ok(format!("Saved screenshot to {}", path.display()))
        }
        "tabs" => {
            let pages = browser
                .pages()
                .await
                .map_err(|e| anyhow!("Cannot list tabs: {e}"))?;
            let mut out = String::new();
            for (i, page) in pages.iter().enumerate() {
                let url = page.url().await.ok().flatten().unwrap_or_default();
                let title = page.get_title().await.ok().flatten().unwrap_or_default();
                out.push_str(&format!("{}. {title} — {url}\n", i + 1));
            }
            if out.is_empty() {
                out.push_str("No tabs.");
            }
            Ok(out)
        }
        other => bail!("Unknown action '{other}'"),
    }
}

// ── Firefox (WebDriver BiDi) ──────────────────────────────────────────────

fn firefox_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        out.push(p.join("Mozilla Firefox/firefox.exe"));
    }
    if let Some(p) = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from) {
        out.push(p.join("Mozilla Firefox/firefox.exe"));
    }
    out.push(PathBuf::from("/usr/bin/firefox"));
    out.push(PathBuf::from("/Applications/Firefox.app/Contents/MacOS/firefox"));
    out
}

/// The debug browser werk launched, so a wedged session can be restarted.
static FIREFOX_CHILD: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

fn firefox_child_alive() -> bool {
    let Ok(mut slot) = FIREFOX_CHILD.lock() else {
        return false;
    };
    match slot.as_mut() {
        Some(child) => child.try_wait().ok().flatten().is_none(),
        None => false,
    }
}

fn kill_firefox_child() {
    let Ok(mut slot) = FIREFOX_CHILD.lock() else {
        return;
    };
    if let Some(mut child) = slot.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn wait_for_port_closed(port: u16) {
    for _ in 0..20 {
        if !port_open(port) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn ensure_firefox(user_profile: bool) -> Result<()> {
    if port_open(FIREFOX_PORT) {
        // Switching to the user profile replaces our isolated instance.
        if user_profile && firefox_child_alive() {
            kill_firefox_child();
            wait_for_port_closed(FIREFOX_PORT);
        } else {
            return Ok(());
        }
    }
    let exe = firefox_candidates()
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("Firefox executable not found"))?;
    if user_profile {
        return launch_user_firefox(&exe);
    }
    let profile = crate::system::temp_workspace()
        .join("browser")
        .join("firefox-profile");
    std::fs::create_dir_all(&profile)?;
    let child = Command::new(&exe)
        .arg("--remote-debugging-port")
        .arg(FIREFOX_PORT.to_string())
        .arg("--profile")
        .arg(&profile)
        .arg("--no-remote")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Cannot launch {}: {e}", exe.display()))?;
    if let Ok(mut slot) = FIREFOX_CHILD.lock() {
        *slot = Some(child);
    }
    wait_for_port(FIREFOX_PORT, "Firefox")
}

/// The user's default profile. Only starts when Firefox is not already
/// running: otherwise the new process hands the port flag to the existing
/// instance and exits without ever opening the debug port.
fn launch_user_firefox(exe: &std::path::Path) -> Result<()> {
    let mut child = Command::new(exe)
        .arg("--remote-debugging-port")
        .arg(FIREFOX_PORT.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Cannot launch {}: {e}", exe.display()))?;
    for _ in 0..60 {
        if port_open(FIREFOX_PORT) {
            return Ok(());
        }
        if child.try_wait().ok().flatten().is_some() {
            bail!(
                "Firefox is already running — quit it completely (including background \
                 windows) and try again, or turn off 'Use my Firefox profile'"
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("Firefox did not open its debug port")
}

/// Kill the debug Firefox werk launched (if any) and start a fresh one.
fn restart_firefox() -> Result<()> {
    kill_firefox_child();
    wait_for_port_closed(FIREFOX_PORT);
    ensure_firefox(false)
}

type BidiSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn firefox_socket() -> Result<BidiSocket> {
    let (ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{FIREFOX_PORT}/session"))
            .await
            .map_err(|e| anyhow!("Cannot attach to Firefox: {e}"))?;
    Ok(ws)
}

/// Connect and create the BiDi session; a stale session or a wedged debug
/// instance is cleaned up by restarting the browser once. The user's own
/// browser is never restarted — it may hold their real windows.
async fn firefox_connect(user_profile: bool) -> Result<(BidiSocket, u64)> {
    let mut ws = firefox_socket().await?;
    let mut id = 0u64;
    match bidi_call(&mut ws, &mut id, "session.new", json!({"capabilities": {}})).await {
        Ok(_) => Ok((ws, id)),
        Err(first) if user_profile => Err(anyhow!("Cannot start a Firefox automation session: {first}")),
        Err(first) => {
            drop(ws);
            restart_firefox()?;
            let mut ws = firefox_socket().await?;
            let mut id = 0u64;
            bidi_call(&mut ws, &mut id, "session.new", json!({"capabilities": {}}))
                .await
                .map_err(|e| {
                    anyhow!(
                        "Cannot start a Firefox automation session: {first}; after a restart: {e}"
                    )
                })?;
            Ok((ws, id))
        }
    }
}

async fn firefox(action: &str, args: &Value, user_profile: bool) -> Result<String> {
    ensure_firefox(user_profile)?;
    let (mut ws, mut id) = firefox_connect(user_profile).await?;
    // Every path must end the session: Firefox allows only one at a time.
    let result = firefox_session(&mut ws, &mut id, action, args).await;
    let _ = bidi_call(&mut ws, &mut id, "session.end", json!({})).await;
    let _ = ws.close(None).await;
    result
}

/// One automation session on an open socket.
async fn firefox_session(
    mut ws: &mut BidiSocket,
    mut id: &mut u64,
    action: &str,
    args: &Value,
) -> Result<String> {
    let context = firefox_context(ws, id).await?;
    match action {
        "open" => {
            let url = req(args, "url")?;
            bidi_call(
                ws,
                id,
                "browsingContext.navigate",
                json!({"context": context, "url": url, "wait": "complete"}),
            )
            .await?;
            Ok(format!("Opened {url}."))
        }
        "read" => {
            let value = bidi_eval(
                ws,
                id,
                &context,
                "document.body ? document.body.innerText : ''",
            )
            .await?;
            Ok(cap(value.as_str().unwrap_or_default().to_string(), 20_000))
        }
        "eval" => {
            let script = req(args, "script")?;
            let value = bidi_eval(ws, id, &context, script).await?;
            Ok(cap(
                serde_json::to_string_pretty(&value).unwrap_or_default(),
                20_000,
            ))
        }
        "click" => {
            let selector = req(args, "selector")?;
            let script = format!(
                "(() => {{ const el = document.querySelector({sel}); if (!el) return false; \
                 el.scrollIntoView({{block:'center'}}); el.click(); return true; }})()",
                sel = json_string(selector)
            );
            let value = bidi_eval(ws, id, &context, &script).await?;
            if value.as_bool() != Some(true) {
                bail!("No element matches '{selector}'");
            }
            Ok(format!("Clicked {selector}."))
        }
        "type" => {
            let selector = req(args, "selector")?;
            let text = req(args, "text")?;
            let script = format!(
                "(() => {{ const el = document.querySelector({sel}); if (!el) return false; \
                 el.focus(); el.value = {text}; \
                 el.dispatchEvent(new Event('input', {{bubbles:true}})); \
                 el.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()",
                sel = json_string(selector),
                text = json_string(text)
            );
            let value = bidi_eval(ws, id, &context, &script).await?;
            if value.as_bool() != Some(true) {
                bail!("No element matches '{selector}'");
            }
            Ok(format!("Typed {} characters into {selector}.", text.chars().count()))
        }
        "screenshot" => {
            let shot = bidi_call(
                ws,
                id,
                "browsingContext.captureScreenshot",
                json!({"context": context}),
            )
            .await?;
            let data = shot
                .pointer("/result/data")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Firefox returned no screenshot data"))?;
            let bytes = crate::roleplay::decode_base64(data).map_err(anyhow::Error::msg)?;
            let path = screenshot_path()?;
            std::fs::write(&path, bytes)?;
            Ok(format!("Saved screenshot to {}", path.display()))
        }
        "tabs" => {
            let tree = bidi_call(ws, id, "browsingContext.getTree", json!({})).await?;
            let mut out = String::new();
            if let Some(contexts) = tree.pointer("/result/contexts").and_then(Value::as_array) {
                for (i, c) in contexts.iter().filter(|c| c.get("parent").is_none()).enumerate() {
                    let url = c.get("url").and_then(Value::as_str).unwrap_or("");
                    out.push_str(&format!("{}. {url}\n", i + 1));
                }
            }
            if out.is_empty() {
                out.push_str("No tabs.");
            }
            Ok(out)
        }
        other => bail!("Unknown action '{other}'"),
    }
}

/// The first top-level tab; one is created when Firefox has none.
async fn firefox_context(ws: &mut BidiSocket, id: &mut u64) -> Result<String> {
    let tree = bidi_call(ws, id, "browsingContext.getTree", json!({})).await?;
    if let Some(context) = tree
        .pointer("/result/contexts")
        .and_then(Value::as_array)
        .and_then(|contexts| {
            contexts
                .iter()
                .find(|c| c.get("parent").is_none())
                .and_then(|c| c.get("context"))
                .and_then(Value::as_str)
        })
    {
        return Ok(context.to_string());
    }
    let created = bidi_call(ws, id, "browsingContext.create", json!({"type": "tab"})).await?;
    created
        .pointer("/result/context")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("Firefox did not create a tab"))
}

async fn bidi_call(
    ws: &mut BidiSocket,
    id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    use futures::SinkExt;
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    *id += 1;
    let call_id = *id;
    let message = json!({"id": call_id, "method": method, "params": params});
    ws.send(Message::Text(message.to_string().into()))
        .await
        .map_err(|e| anyhow!("Cannot send to Firefox: {e}"))?;
    loop {
        let Some(frame) = ws.next().await else {
            bail!("Firefox closed the connection");
        };
        let frame = frame.map_err(|e| anyhow!("Firefox connection error: {e}"))?;
        let Message::Text(text) = frame else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if value.get("id").and_then(Value::as_u64) != Some(call_id) {
            continue; // an event, not our reply
        }
        if let Some(error) = value.get("error") {
            let message = value.get("message").and_then(Value::as_str).unwrap_or("");
            bail!("Firefox rejected {method}: {error} — {message}");
        }
        return Ok(value);
    }
}

async fn bidi_eval(ws: &mut BidiSocket, id: &mut u64, context: &str, script: &str) -> Result<Value> {
    let reply = bidi_call(
        ws,
        id,
        "script.evaluate",
        json!({
            "expression": script,
            "target": {"context": context},
            "awaitPromise": true,
            "resultOwnership": "none",
        }),
    )
    .await?;
    Ok(reply
        .pointer("/result/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}
