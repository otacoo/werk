//! One language server over stdio: JSON-RPC framing, request map, doc sync,
//! and pushed diagnostics. Blocking on purpose: tools run on the blocking pool.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::servers::ServerSpec;
use super::uri;

struct Diag {
    gen: u64,
    items: Vec<Value>,
}

#[derive(Default)]
struct Docs {
    versions: HashMap<String, i64>,
    diagnostics: HashMap<String, Diag>,
    /// Global generation when the document was last opened/changed. Waits
    /// compare against this, not against call time: a publish that races
    /// ahead of `wait_diagnostics` must still count as new.
    change_gen: HashMap<String, u64>,
    gen: u64,
}

pub struct Client {
    stdin: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Option<Child>>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    docs: Mutex<Docs>,
    cv: Condvar,
    alive: AtomicBool,
}

impl Client {
    /// Spawn the spec's program for `root` and complete the initialize handshake.
    pub fn spawn(spec: &ServerSpec, binary: &Path, root: &Path) -> Result<Arc<Client>> {
        let args = spec.command.get(1..).unwrap_or_default();
        let mut cmd = spawn_command(binary, args);
        cmd.current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            #[allow(unused_imports)]
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to start {}", spec.name))?;
        let stdin = child.stdin.take().context("server has no stdin")?;
        let stdout = child.stdout.take().context("server has no stdout")?;
        let client = Self::with_io(Box::new(stdin), Box::new(stdout));
        *client.child.lock().unwrap() = Some(child);
        if let Err(e) = client.initialize(&spec.name, root) {
            client.kill();
            return Err(e);
        }
        Ok(client)
    }

    /// Transport-agnostic core: any pair of byte streams will do.
    fn with_io(stdin: Box<dyn Write + Send>, reader: Box<dyn Read + Send>) -> Arc<Client> {
        let client = Arc::new(Client {
            stdin: Mutex::new(stdin),
            child: Mutex::new(None),
            next_id: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            docs: Mutex::new(Docs::default()),
            cv: Condvar::new(),
            alive: AtomicBool::new(true),
        });
        Self::reader_thread(&client, reader);
        client
    }

    fn initialize(&self, server_id: &str, root: &Path) -> Result<()> {
        let root_uri = uri::path_to_uri(root);
        self.request(
            "initialize",
            json!({
                "processId": null,
                "clientInfo": {"name": "werk"},
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": "project"}],
                "capabilities": {
                    "textDocument": {
                        "synchronization": {"dynamicRegistration": false},
                        "publishDiagnostics": {"versionSupport": true},
                        "definition": {},
                        "implementation": {},
                        "references": {},
                        "hover": {"contentFormat": ["markdown", "plaintext"]},
                        "documentSymbol": {"hierarchicalDocumentSymbolSupport": true}
                    },
                    "workspace": {"symbol": {}, "configuration": true, "workspaceFolders": true}
                }
            }),
            Duration::from_secs(30),
        )
        .with_context(|| format!("{server_id} failed to initialize"))?;
        self.notify("initialized", json!({}))
    }

    fn reader_thread(client: &Arc<Client>, reader: Box<dyn Read + Send>) {
        let weak = Arc::downgrade(client);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            while let Ok(Some(msg)) = read_frame(&mut reader) {
                let Some(client) = weak.upgrade() else { return };
                client.dispatch(msg);
            }
            if let Some(client) = weak.upgrade() {
                client.shutdown();
            }
        });
    }

    fn dispatch(&self, msg: Value) {
        match (msg.get("id"), msg.get("method").and_then(Value::as_str)) {
            // Server-initiated request: answer, don't negotiate.
            (Some(id), Some(method)) => {
                let result = if method == "workspace/configuration" {
                    let n = msg
                        .pointer("/params/items")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    Value::Array(vec![Value::Null; n])
                } else {
                    Value::Null
                };
                let _ = self.write(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            (None, Some("textDocument/publishDiagnostics")) => {
                let params = &msg["params"];
                let Some(uri) = params.get("uri").and_then(Value::as_str) else {
                    return;
                };
                let items = params
                    .get("diagnostics")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut docs = self.docs.lock().unwrap();
                docs.gen += 1;
                let gen = docs.gen;
                docs.diagnostics
                    .insert(uri.to_string(), Diag { gen, items });
                self.cv.notify_all();
            }
            // Response to one of our requests.
            (Some(id), None) => {
                if let Some(tx) = self.pending.lock().unwrap().remove(&id.as_i64().unwrap_or(-1)) {
                    let _ = tx.send(msg);
                }
            }
            _ => {}
        }
    }

    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.write(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        })) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let msg = match rx.recv_timeout(timeout) {
            Ok(msg) => msg,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                bail!("{method} timed out after {}s", timeout.as_secs());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("{method}: server stopped"),
        };
        if let Some(err) = msg.get("error") {
            let text = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("{method} failed: {text}");
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn write(&self, msg: &Value) -> Result<()> {
        let mut stdin = self.stdin.lock().unwrap();
        write_frame(&mut *stdin, msg)?;
        Ok(())
    }

    /// didOpen on first sight, didChange afterwards (full-text sync).
    pub fn open_or_change(&self, uri: &str, language: &str, text: &str) {
        enum Action {
            Open,
            Change(i64),
        }
        let action = {
            let mut docs = self.docs.lock().unwrap();
            let gen = docs.gen;
            docs.change_gen.insert(uri.to_string(), gen);
            match docs.versions.get_mut(uri) {
                Some(version) => {
                    *version += 1;
                    Action::Change(*version)
                }
                None => {
                    docs.versions.insert(uri.to_string(), 1);
                    Action::Open
                }
            }
        };
        let _ = match action {
            Action::Open => self.notify(
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": uri, "languageId": language, "version": 1, "text": text
                }}),
            ),
            Action::Change(version) => self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": uri, "version": version},
                    "contentChanges": [{"text": text}]
                }),
            ),
        };
    }

    /// Open with on-disk text unless already tracked.
    pub fn ensure_open(&self, uri: &str, language: &str, text: &str) {
        if !self.docs.lock().unwrap().versions.contains_key(uri) {
            self.open_or_change(uri, language, text);
        }
    }

    /// Diagnostics published after the last change. A short settle window lets
    /// a semantic pass supersede the first (often syntax-only) publish.
    pub fn wait_diagnostics(&self, uri: &str, timeout: Duration) -> Option<Vec<Value>> {
        let deadline = Instant::now() + timeout;
        let mut docs = self.docs.lock().unwrap();
        let baseline = docs.change_gen.get(uri).copied().unwrap_or(docs.gen);
        let arrived = |d: &Docs| d.diagnostics.get(uri).is_some_and(|g| g.gen > baseline);
        while !arrived(&docs) {
            if !self.alive.load(Ordering::SeqCst) {
                return None;
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, _) = self.cv.wait_timeout(docs, deadline - now).unwrap();
            docs = guard;
        }
        let settle = Duration::from_millis(150);
        let (guard, _) = self.cv.wait_timeout(docs, settle).unwrap();
        docs = guard;
        docs.diagnostics.get(uri).map(|d| d.items.clone())
    }

    fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.pending.lock().unwrap().clear();
        self.cv.notify_all();
    }

    pub fn kill(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        self.shutdown();
    }
}

/// Windows npm shims are `.cmd`/`.bat` — those need the shell.
fn spawn_command(binary: &Path, args: &[String]) -> Command {
    #[cfg(windows)]
    {
        let shim = binary
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
        if shim {
            let mut cmd = Command::new("cmd");
            cmd.arg("/C").arg(binary).args(args);
            return cmd;
        }
    }
    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd
}

fn write_frame(writer: &mut impl Write, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// `Content-Length`-framed JSON-RPC message; `None` at clean EOF.
fn read_frame<R: BufRead>(reader: &mut R) -> std::io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            len = value.trim().parse().ok();
        }
    }
    let len = len.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing Content-Length")
    })?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf).unwrap_or(Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frames_parse_back_to_back() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}";
        let raw = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}{}{}",
            body.len(),
            body,
            format!("Content-Length: {}\r\n\r\n[\"a\"]", 5),
            ""
        );
        let mut cursor = Cursor::new(raw.into_bytes());
        let first = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(first["id"], 1);
        let second = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(second[0], "a");
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn missing_length_is_an_error() {
        let mut cursor = Cursor::new(b"Content-Type: x\r\n\r\n{}".to_vec());
        assert!(read_frame(&mut cursor).is_err());
    }

    /// Byte-channel mock so the whole client loop is exercised without a process.
    struct ChanReader(mpsc::Receiver<Vec<u8>>, Vec<u8>);

    impl Read for ChanReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(chunk) => self.1 = chunk,
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len());
            buf[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }

    struct ChanWriter(mpsc::Sender<Vec<u8>>);

    impl Write for ChanWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let send = self.0.send(buf.to_vec());
            send.map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "mock closed"))?;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn speaks_the_protocol_end_to_end() {
        let (to_server_tx, to_server_rx) = mpsc::channel::<Vec<u8>>();
        let (to_client_tx, to_client_rx) = mpsc::channel::<Vec<u8>>();
        let (config_seen_tx, config_seen_rx) = mpsc::channel::<()>();

        std::thread::spawn(move || {
            let mut reader = BufReader::new(ChanReader(to_server_rx, Vec::new()));
            let mut writer = ChanWriter(to_client_tx);
            let mut announced = false;
            while let Ok(Some(msg)) = read_frame(&mut reader) {
                match msg.get("method").and_then(Value::as_str) {
                    Some("initialize") => {
                        write_frame(
                            &mut writer,
                            &json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}}),
                        )
                        .unwrap();
                    }
                    Some("initialized") => {
                        // Server request the client must answer.
                        write_frame(
                            &mut writer,
                            &json!({
                                "jsonrpc": "2.0", "id": 99, "method": "workspace/configuration",
                                "params": {"items": [{}, {}]}
                            }),
                        )
                        .unwrap();
                    }
                    Some("textDocument/didOpen") => {
                        if !announced {
                            announced = true;
                            let uri = msg.pointer("/params/textDocument/uri").unwrap().clone();
                            write_frame(
                                &mut writer,
                                &json!({
                                    "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                                    "params": {"uri": uri, "diagnostics": [{
                                        "severity": 1, "message": "boom",
                                        "range": {"start": {"line": 3, "character": 2}}
                                    }]}
                                }),
                            )
                            .unwrap();
                        }
                    }
                    Some("textDocument/definition") => {
                        write_frame(
                            &mut writer,
                            &json!({"jsonrpc": "2.0", "id": msg["id"], "result": [
                                {"uri": "file:///x.rs", "range": {"start": {"line": 1, "character": 0}}}
                            ]}),
                        )
                        .unwrap();
                    }
                    // didChange is accepted silently (no publish needed).
                    Some("textDocument/didChange") => {}
                    Some(other) => panic!("unexpected method {other}"),
                    // Response from the client (our configuration request).
                    None => {
                        assert_eq!(msg["id"], 99);
                        assert_eq!(msg["result"], json!([null, null]));
                        config_seen_tx.send(()).unwrap();
                    }
                }
            }
        });

        let client = Client::with_io(
            Box::new(ChanWriter(to_server_tx)),
            Box::new(ChanReader(to_client_rx, Vec::new())),
        );
        client.initialize("mock", Path::new("/proj")).unwrap();
        config_seen_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("client must answer workspace/configuration");

        let result = client
            .request("textDocument/definition", json!({}), Duration::from_secs(2))
            .unwrap();
        assert_eq!(result[0]["uri"], "file:///x.rs");

        client.open_or_change("file:///x.rs", "rust", "fn main() {}");
        // Regression: the publish can land before the wait starts (fast/CI
        // machines); it must still count as diagnostics for this change.
        std::thread::sleep(Duration::from_millis(100));
        let diagnostics = client
            .wait_diagnostics("file:///x.rs", Duration::from_secs(2))
            .expect("diagnostics pushed after didOpen");
        assert_eq!(diagnostics[0]["message"], "boom");
        assert!(diagnostics[0]["range"]["start"]["line"].as_u64().is_some());

        // A later change with no new publish must time out, not hang.
        client.open_or_change("file:///x.rs", "rust", "fn main() { }");
        assert!(client.wait_diagnostics("file:///x.rs", Duration::from_millis(300)).is_none());
    }
}
