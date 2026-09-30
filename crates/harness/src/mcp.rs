//! Minimal MCP client over stdio: spawns a server, handshakes, lists tools,
//! and calls them. One JSON-RPC message per line — the same transport the
//! Cursor-compatible `mcp.json` entries describe.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::permissions::ApprovalKey;
use crate::tools::Tool;

const INIT_TIMEOUT: Duration = Duration::from_secs(20);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULT_CHARS: usize = 100_000;

/// One MCP server definition, already shim-wrapped by the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u32>,
}

/// A tool advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub server: String,
    /// Raw tool name as the server knows it.
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Cached tool schemas for one server, tied to its spec fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedServer {
    pub name: String,
    pub fingerprint: u64,
    pub tools: Vec<McpToolDef>,
}

/// Per-server state for the Tools → Live view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    /// Started by werk right now (only happens when a tool is called or the
    /// live check is run explicitly).
    pub running: bool,
    /// Tool names known from the schema cache (empty until first discovery).
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Stable hash of everything that changes a server's tool set.
pub fn spec_fingerprint(spec: &McpServerSpec) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spec.command.hash(&mut h);
    spec.args.hash(&mut h);
    spec.cwd.hash(&mut h);
    let mut env: Vec<_> = spec.env.iter().collect();
    env.sort();
    for (k, v) in env {
        k.hash(&mut h);
        v.hash(&mut h);
    }
    h.finish()
}

/// Name the model sees: `mcp_<server>_<tool>`, sanitized to one word and
/// clamped to the usual 64-character function-name limit.
pub fn exposed_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect()
    };
    let name = format!("mcp_{}_{}", clean(server), clean(tool));
    name.chars().take(64).collect()
}

pub struct McpServer {
    name: String,
    timeout: Duration,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    rx: Mutex<Receiver<(u64, Value)>>,
    next_id: AtomicU64,
}

impl McpServer {
    /// Spawn the server and run the initialize handshake.
    pub fn start(spec: &McpServerSpec) -> Result<McpServer> {
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = spec.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
            cmd.current_dir(cwd);
        }
        #[cfg(target_os = "windows")]
        {
            #[allow(unused_imports)]
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("MCP '{}': cannot spawn {}", spec.name, spec.command))?;
        let stdin = child.stdin.take().context("MCP: no stdin pipe")?;
        let stdout = child.stdout.take().context("MCP: no stdout pipe")?;
        // Drain stderr so a chatty server cannot block on a full pipe.
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                for _ in BufReader::new(stderr).lines() {}
            });
        }
        let (tx, rx) = channel::<(u64, Value)>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    if tx.send((id, v)).is_err() {
                        break;
                    }
                }
            }
        });
        let server = McpServer {
            name: spec.name.clone(),
            timeout: spec
                .timeout_ms
                .map(|ms| Duration::from_millis(ms.max(1000) as u64))
                .unwrap_or(CALL_TIMEOUT),
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            rx: Mutex::new(rx),
            next_id: AtomicU64::new(1),
        };
        server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "werk", "version": env!("CARGO_PKG_VERSION") },
            }),
            INIT_TIMEOUT,
        )?;
        server.notify("notifications/initialized", json!({}))?;
        Ok(server)
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        {
            let mut stdin = self.stdin.lock().unwrap();
            writeln!(stdin, "{msg}").with_context(|| format!("MCP '{}': write failed", self.name))?;
            stdin.flush().ok();
        }
        let rx = self.rx.lock().unwrap();
        let deadline = Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                bail!("MCP '{}': {} timed out", self.name, method);
            };
            match rx.recv_timeout(remaining) {
                Ok((rid, v)) if rid == id => {
                    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                        bail!("MCP '{}': {}", self.name, error_text(err));
                    }
                    return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                }
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => {
                    bail!("MCP '{}': {} timed out", self.name, method)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("MCP '{}': server exited", self.name)
                }
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let mut stdin = self.stdin.lock().unwrap();
        writeln!(stdin, "{msg}").with_context(|| format!("MCP '{}': write failed", self.name))?;
        stdin.flush().ok();
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Tools the server advertises (paginated listings are followed).
    pub fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..8 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let result = self.request("tools/list", params, INIT_TIMEOUT)?;
            out.extend(parse_tools_list(&self.name, &result));
            cursor = result
                .get("nextCursor")
                .and_then(|c| c.as_str())
                .map(str::to_string)
                .filter(|c| !c.is_empty());
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    pub fn call_tool(&self, tool: &str, arguments: &Value) -> Result<String> {
        let args = if arguments.is_null() { json!({}) } else { arguments.clone() };
        let result = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": args }),
            self.timeout,
        )?;
        parse_tool_result(tool, &result)
    }

    pub fn shutdown(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn error_text(err: &Value) -> String {
    let msg = err
        .get("message")
        .and_then(|m| m.as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("unknown error");
    match err.get("code").and_then(|c| c.as_i64()) {
        Some(code) => format!("{msg} (code {code})"),
        None => msg.to_string(),
    }
}

fn parse_tools_list(server: &str, result: &Value) -> Vec<McpToolDef> {
    result
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| {
                    let name = t.get("name").and_then(|n| n.as_str())?.to_string();
                    let description = t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input_schema = t
                        .get("inputSchema")
                        .filter(|s| s.is_object())
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
                    // Providers require an object-typed parameter schema.
                    let mut input_schema = input_schema;
                    if input_schema.get("type").is_none() {
                        input_schema["type"] = json!("object");
                    }
                    Some(McpToolDef { server: server.to_string(), name, description, input_schema })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_tool_result(tool: &str, result: &Value) -> Result<String> {
    let is_error = result.get("isError").and_then(|b| b.as_bool()).unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    match result.get("content") {
        Some(Value::Array(items)) => {
            for item in items {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            parts.push(text.to_string());
                        }
                    }
                    Some("image") => {
                        let mime = item
                            .get("mimeType")
                            .and_then(|m| m.as_str())
                            .unwrap_or("image");
                        parts.push(format!("[image: {mime}]"));
                    }
                    Some("resource") => {
                        let uri = item
                            .get("resource")
                            .and_then(|r| r.get("uri"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("resource");
                        parts.push(format!("[resource: {uri}]"));
                    }
                    _ => {}
                }
            }
        }
        Some(Value::String(text)) => parts.push(text.clone()),
        _ => {}
    }
    let mut text = parts.join("\n").trim().to_string();
    if text.chars().count() > MAX_RESULT_CHARS {
        let head: String = text.chars().take(MAX_RESULT_CHARS).collect();
        text = format!("{head}\n[…truncated]");
    }
    if is_error {
        bail!("MCP tool '{tool}' failed: {text}");
    }
    Ok(if text.is_empty() { "(no output)".to_string() } else { text })
}

/// One server, started only on first tool use — never just to list tools.
struct LazyServer {
    spec: McpServerSpec,
    server: Mutex<Option<Arc<McpServer>>>,
}

impl LazyServer {
    fn new(spec: McpServerSpec) -> Self {
        Self { spec, server: Mutex::new(None) }
    }

    fn fingerprint(&self) -> u64 {
        spec_fingerprint(&self.spec)
    }

    /// Start the server if it is not running yet.
    fn get(&self) -> Result<Arc<McpServer>> {
        let mut guard = self.server.lock().unwrap();
        if let Some(server) = guard.as_ref() {
            return Ok(server.clone());
        }
        let server = Arc::new(McpServer::start(&self.spec)?);
        *guard = Some(server.clone());
        Ok(server)
    }

    fn running(&self) -> bool {
        self.server.lock().unwrap().is_some()
    }

    fn shutdown(&self) {
        if let Some(server) = self.server.lock().unwrap().take() {
            server.shutdown();
        }
    }
}

/// A registered MCP tool; starts its server on first use and is gated
/// through the permission engine by default.
pub struct McpTool {
    lazy: Arc<LazyServer>,
    def: McpToolDef,
    name: String,
    approval: bool,
}

impl McpTool {
    fn new(lazy: Arc<LazyServer>, def: McpToolDef, approval: bool) -> Self {
        let name = exposed_name(&def.server, &def.name);
        Self { lazy, def, name, approval }
    }

    pub fn server_name(&self) -> &str {
        &self.def.server
    }

    pub fn tool_name(&self) -> &str {
        &self.def.name
    }
}

impl Tool for McpTool {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn description(&self) -> String {
        let desc = if self.def.description.trim().is_empty() {
            "MCP tool".to_string()
        } else {
            self.def.description.trim().to_string()
        };
        format!("[MCP: {}] {desc}", self.def.server)
    }

    fn parameters(&self) -> Value {
        self.def.input_schema.clone()
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        self.approval.then(|| ApprovalKey { tool: self.name.clone(), command: None })
    }

    fn execute(&self, args: &Value) -> Result<String> {
        self.lazy.get()?.call_tool(&self.def.name, args)
    }
}

/// Persistent set of MCP servers with lazily started children and a schema
/// cache, so runs advertise tools without spawning anything.
pub struct McpManager {
    servers: Vec<Arc<LazyServer>>,
    approval: bool,
    cache: Mutex<HashMap<String, CachedServer>>,
    errors: Mutex<HashMap<String, String>>,
}

impl McpManager {
    pub fn new(
        specs: Vec<McpServerSpec>,
        approval: bool,
        cache: Vec<CachedServer>,
    ) -> Self {
        let cached: HashMap<String, CachedServer> =
            cache.into_iter().map(|c| (c.name.clone(), c)).collect();
        Self {
            servers: specs.into_iter().map(|s| Arc::new(LazyServer::new(s))).collect(),
            approval,
            cache: Mutex::new(cached),
            errors: Mutex::new(HashMap::new()),
        }
    }

    /// Tools to advertise: cached schemas when the spec is unchanged (no
    /// process starts), otherwise start once to discover and cache.
    pub fn tools(&self) -> (Vec<Arc<McpTool>>, Vec<String>) {
        let mut tools = Vec::new();
        let mut errors = Vec::new();
        for server in &self.servers {
            let cached = {
                let cache = self.cache.lock().unwrap();
                cache
                    .get(&server.spec.name)
                    .filter(|c| c.fingerprint == server.fingerprint())
                    .map(|c| c.tools.clone())
            };
            match cached {
                Some(defs) => {
                    for def in defs {
                        tools.push(Arc::new(McpTool::new(server.clone(), def, self.approval)));
                    }
                }
                None => match self.discover(server) {
                    Ok(defs) => {
                        for def in defs {
                            tools.push(Arc::new(McpTool::new(server.clone(), def, self.approval)));
                        }
                    }
                    Err(e) => errors.push(e),
                },
            }
        }
        (tools, errors)
    }

    /// Force (re)discovery for every server — the Live view's "Check now".
    pub fn refresh(&self) -> (Vec<Arc<McpTool>>, Vec<String>) {
        let mut tools = Vec::new();
        let mut errors = Vec::new();
        for server in &self.servers {
            match self.discover(server) {
                Ok(defs) => {
                    for def in defs {
                        tools.push(Arc::new(McpTool::new(server.clone(), def, self.approval)));
                    }
                }
                Err(e) => errors.push(e),
            }
        }
        (tools, errors)
    }

    fn discover(&self, server: &Arc<LazyServer>) -> Result<Vec<McpToolDef>, String> {
        let result = server
            .get()
            .and_then(|client| client.list_tools())
            .map_err(|e| format!("{e:#}"));
        match result {
            Ok(defs) => {
                self.errors.lock().unwrap().remove(&server.spec.name);
                self.cache.lock().unwrap().insert(
                    server.spec.name.clone(),
                    CachedServer {
                        name: server.spec.name.clone(),
                        fingerprint: server.fingerprint(),
                        tools: defs.clone(),
                    },
                );
                Ok(defs)
            }
            Err(e) => {
                self.errors
                    .lock()
                    .unwrap()
                    .insert(server.spec.name.clone(), e.clone());
                Err(e)
            }
        }
    }

    /// Known state without starting anything (Live view on open).
    pub fn cached_status(&self) -> Vec<McpServerStatus> {
        let cache = self.cache.lock().unwrap();
        let errors = self.errors.lock().unwrap();
        self.servers
            .iter()
            .map(|s| {
                let tools = cache
                    .get(&s.spec.name)
                    .filter(|c| c.fingerprint == s.fingerprint())
                    .map(|c| c.tools.iter().map(|t| t.name.clone()).collect())
                    .unwrap_or_default();
                McpServerStatus {
                    name: s.spec.name.clone(),
                    running: s.running(),
                    tools,
                    error: errors.get(&s.spec.name).cloned(),
                }
            })
            .collect()
    }

    /// Start and (re)list every server, returning the same shape (Live view
    /// "Check now" — the only path that starts servers without a tool call).
    pub fn probe_status(&self) -> Vec<McpServerStatus> {
        self.refresh();
        self.cached_status()
    }

    /// Snapshot for persistence (only entries matching current specs).
    pub fn cache_snapshot(&self) -> Vec<CachedServer> {
        let cache = self.cache.lock().unwrap();
        self.servers
            .iter()
            .filter_map(|s| {
                cache
                    .get(&s.spec.name)
                    .filter(|c| c.fingerprint == s.fingerprint())
                    .cloned()
            })
            .collect()
    }

    pub fn shutdown(&self) {
        for server in &self.servers {
            server.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposed_names_are_single_words() {
        assert_eq!(exposed_name("ddg-search", "search.web"), "mcp_ddg-search_search_web");
        assert_eq!(exposed_name("my server", "t/1"), "mcp_my_server_t_1");
    }

    #[test]
    fn tools_list_parses_and_tolerates_junk() {
        let result = json!({
            "tools": [
                { "name": "search", "description": "Search the web", "inputSchema": { "type": "object" } },
                { "name": "broken" },
                "junk"
            ]
        });
        let defs = parse_tools_list("ddg", &result);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "search");
        assert_eq!(defs[1].input_schema["type"], "object");
        assert!(parse_tools_list("x", &json!({})).is_empty());
    }

    #[test]
    fn tool_results_flatten_content_parts() {
        let result = json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "image", "mimeType": "image/png" },
                { "type": "resource", "resource": { "uri": "file://a" } }
            ]
        });
        let text = parse_tool_result("t", &result).unwrap();
        assert!(text.contains("hello") && text.contains("image/png") && text.contains("file://a"));
        assert_eq!(parse_tool_result("t", &json!({})).unwrap(), "(no output)");
    }

    #[test]
    fn cached_schemas_avoid_starting_servers() {
        let spec = McpServerSpec {
            name: "dead".into(),
            command: "definitely-not-a-real-binary-xyz".into(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            timeout_ms: None,
        };
        let def = McpToolDef {
            server: "dead".into(),
            name: "ping".into(),
            description: "Ping".into(),
            input_schema: json!({"type": "object"}),
        };
        let cached = CachedServer {
            name: "dead".into(),
            fingerprint: spec_fingerprint(&spec),
            tools: vec![def],
        };
        let manager = McpManager::new(vec![spec.clone()], true, vec![cached]);
        let (tools, errors) = manager.tools();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "mcp_dead_ping");
        let status = manager.cached_status();
        assert_eq!(status.len(), 1);
        assert!(!status[0].running);
        assert_eq!(status[0].tools, vec!["ping"]);
        assert_eq!(manager.cache_snapshot().len(), 1);

        // A stale fingerprint forces a real discovery, which fails here.
        let stale = CachedServer { name: "dead".into(), fingerprint: 0, tools: vec![] };
        let manager = McpManager::new(vec![spec], true, vec![stale]);
        let (_, errors) = manager.tools();
        assert!(!errors.is_empty());
    }

    #[test]
    fn tool_errors_surface() {
        let err = parse_tool_result("t", &json!({ "isError": true, "content": [{"type":"text","text":"boom"}] }));
        assert!(err.unwrap_err().to_string().contains("boom"));
        assert!(error_text(&json!({"code": -32601, "message": "Method not found"})).contains("Method not found"));
    }
}
