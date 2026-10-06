//! OpenAI-compatible chat client with streaming tool-call support.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

// ── Messages ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Some(Value::String(content.into())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(Value::String(content.into())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: Some(Value::String(content.into())),
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// ── Streaming events ──────────────────────────────────────────────────────

/// Chat-control strings some models emit as text on template mismatch.
/// Scrubbed from assistant text only — never tool results or user content.
pub fn strip_special_tokens(text: &str) -> String {
    let mut out = text.to_string();
    for tok in [
        "<|im_start|>",
        "<|im_end|>",
        "<|endoftext|>",
        "<|end_of_text|>",
        "<|eot_id|>",
        "<|eot|>",
        "<|end|>",
        "<|start_header_id|>",
        "<|end_header_id|>",
        "<|begin_of_sentence|>",
        "<|end_of_sentence|>",
        "<|beginofsentence|>",
        "<|endofsentence|>",
        "<｜begin_of_sentence｜>",
        "<｜end_of_sentence｜>",
        "<｜begin▁of▁sentence｜>",
        "<｜end▁of▁sentence｜>",
        "<|begin▁of▁sentence|>",
        "<|end▁of▁sentence|>",
        "[INST]",
        "[/INST]",
        "<<SYS>>",
        "<</SYS>>",
    ] {
        out = out.replace(tok, "");
    }
    // Bare BOS/EOS markers only at the boundaries — they can be legit
    // mid-text (e.g. discussing tokenizer tokens).
    loop {
        let t = out.trim();
        let s = t
            .strip_prefix("<s>")
            .or_else(|| t.strip_prefix("<bos>"))
            .or_else(|| t.strip_suffix("</s>"))
            .or_else(|| t.strip_suffix("<eos>"));
        match s {
            Some(rest) => out = rest.to_string(),
            None => break,
        }
    }
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out.trim().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Content { text: String },
    ReasoningDelta { text: String },
    ToolCallDelta {
        index: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default)]
        arguments_delta: String,
    },
    Finish { reason: String },
    Usage { prompt_tokens: u64, completion_tokens: u64 },
    Notice { text: String },
}

/// Assembles deltas into complete tool calls; args repaired via `jsonfix`.
#[derive(Debug, Default)]
pub struct StreamCollector {
    calls: Vec<PartialToolCall>,
}

#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl StreamCollector {
    pub fn push(&mut self, ev: &StreamEvent) {
        if let StreamEvent::ToolCallDelta { index, id, name, arguments_delta } = ev {
            while self.calls.len() <= *index {
                self.calls.push(PartialToolCall::default());
            }
            let call = &mut self.calls[*index];
            if let Some(id) = id {
                call.id.push_str(id);
            }
            if let Some(name) = name {
                call.name.push_str(name);
            }
            call.arguments.push_str(arguments_delta);
        }
    }

    /// Repaired args; still fails loud when unparseable.
    pub fn finish(&self) -> Result<Vec<ToolCall>> {
        self.calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let arguments = if c.arguments.trim().is_empty() {
                    "{}".to_string()
                } else {
                    let repaired = crate::jsonfix::repair_json(&c.arguments);
                    if serde_json::from_str::<Value>(&repaired).is_err() {
                        bail!(
                            "Tool call {} ({}) has unparseable arguments: {:?}",
                            i,
                            c.name,
                            c.arguments
                        );
                    }
                    repaired
                };
                Ok(ToolCall {
                    id: c.id.clone(),
                    call_type: "function".to_string(),
                    function: FunctionCall { name: c.name.clone(), arguments },
                })
            })
            .collect()
    }
}

/// One SSE `data:` line (without the prefix) → event, if mappable.
fn parse_sse_line(line: &str) -> Option<StreamEvent> {
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(StreamEvent::Finish { reason: "stop".to_string() });
    }
    let v: Value = serde_json::from_str(data).ok()?;
    // A 200 stream can still terminate with an error payload (OpenRouter
    // forwards provider errors this way).
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return Some(StreamEvent::Notice { text: provider_error_text(err) });
    }
    let choice = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first());
    if let Some(choice) = choice {
        if let Some(err) = choice.get("error").filter(|e| !e.is_null()) {
            return Some(StreamEvent::Notice { text: provider_error_text(err) });
        }
        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                if !text.is_empty() {
                    return Some(StreamEvent::Content { text: text.to_string() });
                }
            }
            if let Some(text) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(|c| c.as_str())
            {
                if !text.is_empty() {
                    return Some(StreamEvent::ReasoningDelta { text: text.to_string() });
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                // First call chunk per line is enough; collectors merge by index.
                let call = calls.first()?;
                let index = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let id = call.get("id").and_then(|s| s.as_str()).map(String::from);
                let name = call
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(String::from);
                let args = call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_string();
                return Some(StreamEvent::ToolCallDelta { index, id, name, arguments_delta: args });
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
            if !reason.is_empty() {
                return Some(StreamEvent::Finish { reason: reason.to_string() });
            }
        }
    }
    // Usage arrives in a final chunk; OpenAI sends it with an empty `choices`.
    if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
        let prompt = usage.get("prompt_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
        let gen = usage.get("completion_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
        return Some(StreamEvent::Usage { prompt_tokens: prompt, completion_tokens: gen });
    }
    None
}

fn provider_error_text(err: &Value) -> String {
    match err.get("code").and_then(|c| c.as_u64()) {
        Some(code) => format!("Provider error {code}: {}", describe_error(err)),
        None => format!("Provider error: {}", describe_error(err)),
    }
}

/// Human-readable error object: message, plus OpenRouter's forwarded
/// `metadata.raw` (the actionable upstream reason) and provider name.
fn describe_error(err: &Value) -> String {
    let msg = err
        .get("message")
        .and_then(|m| m.as_str())
        .or_else(|| err.as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("unknown provider error");
    let raw = err
        .get("metadata")
        .and_then(|m| m.get("raw"))
        .and_then(|r| r.as_str())
        .map(str::trim)
        .filter(|r| !r.is_empty() && *r != msg);
    let provider = err
        .get("metadata")
        .and_then(|m| m.get("provider_name"))
        .and_then(|p| p.as_str())
        .filter(|p| !p.is_empty());
    let mut out = msg.to_string();
    if let Some(raw) = raw {
        out.push_str(" — ");
        out.extend(raw.chars().take(300));
    }
    if let Some(provider) = provider {
        out.push_str(&format!(" ({provider})"));
    }
    out
}

/// Messages from the dedicated-runtime request worker.
enum WorkerMsg {
    Event(StreamEvent),
    Error(String),
    Done,
}

/// API base normalization: SDK-style `…/v1` and bare roots both resolve to
/// `{base}/v1/…` (chat) and `{base}/v1/models` (listing).
pub fn normalize_base(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    base.strip_suffix("/v1")
        .unwrap_or(base)
        .trim_end_matches('/')
        .to_string()
}

/// OpenAI/OpenRouter-style errors carry `{"error": {"message": …}}`; fall
/// back to the raw body when it is not JSON.
pub fn http_error_detail(status: u16, body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let err = v.get("error");
        let message = err
            .and_then(|e| {
                e.get("message")
                    .and_then(|m| m.as_str())
                    .or_else(|| e.as_str())
            })
            .or_else(|| v.get("message").and_then(|m| m.as_str()))
            .map(str::trim)
            .filter(|m| !m.is_empty());
        if let Some(msg) = message {
            let detail = match err {
                Some(err) if !err.is_null() => describe_error(err),
                _ => msg.to_string(),
            };
            let code = err
                .and_then(|e| e.get("code"))
                .or_else(|| v.get("code"))
                .map(|c| match c {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            return match code {
                Some(code) => format!("status {status}, code {code}: {detail}"),
                None => format!("status {status}: {detail}"),
            };
        }
    }
    let body = body.trim();
    let clipped: String = body.chars().take(500).collect();
    match clipped.len() < body.len() {
        true => format!("status {status}: {clipped}…"),
        false => format!("status {status}: {clipped}"),
    }
}

pub struct LlmClient {
    base_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl LlmClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_key(base_url, None)
    }

    pub fn with_key(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            base_url: normalize_base(&base_url.into()),
            api_key,
            // No timeout: generations can stream for minutes.
            http: reqwest::Client::builder()
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn chat_stream(
        &self,
        model: Option<&str>,
        messages: &[ChatMessage],
        tools: Option<&[Value]>,
        reasoning_effort: Option<&str>,
        sampling: Option<&Value>,
        should_stop: impl Fn() -> bool,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<String> {
        let mut body = serde_json::json!({
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if let Some(m) = model {
            body["model"] = Value::from(m);
        }
        if let Some(effort) = reasoning_effort {
            body["reasoning_effort"] = Value::from(effort);
        }
        if let Some(s) = sampling.and_then(|s| s.as_object()) {
            for (key, value) in s {
                body[key] = value.clone();
            }
        }
        if let Some(t) = tools {
            body["tools"] = Value::from(t.to_vec());
        }
        let url = format!("{}/v1/chat/completions", self.base_url);
        let api_key = self.api_key.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WorkerMsg>();
        // Dropping a reqwest stream keeps the connection alive (hyper reuse),
        // so a stopped run would leave the provider generating. The request
        // runs on its own runtime: when the consumer goes away, the socket
        // closes - which is how OpenAI-compatible providers cancel.
        let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_gone = gone.clone();
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build()
            else {
                let _ = tx.send(WorkerMsg::Error("Chat request failed: runtime setup".into()));
                return;
            };
            rt.block_on(async move {
                let http = reqwest::Client::builder()
                    .build()
                    .expect("failed to build HTTP client");
                let mut request = http.post(&url).json(&body);
                if let Some(key) = &api_key {
                    request = request.bearer_auth(key);
                }
                let resp = match request.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(WorkerMsg::Error(format!("Chat request failed: {e}")));
                        return;
                    }
                };
                if !resp.status().is_success() {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    let _ = tx.send(WorkerMsg::Error(format!(
                        "Chat request failed — {}",
                        http_error_detail(status.as_u16(), &text)
                    )));
                    return;
                }
                use futures::StreamExt;
                let mut stream = resp.bytes_stream();
                let mut buf = String::new();
                // A wedged server (thrashing VRAM, dead slot) must not hang
                // the loop: give up after a long silence.
                const STALL_TIMEOUT: Duration = Duration::from_secs(300);
                let mut ticker =
                    tokio::time::interval(Duration::from_millis(200));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                let mut last_output = std::time::Instant::now();
                loop {
                    tokio::select! {
                        chunk = stream.next() => {
                            let Some(chunk) = chunk else {
                                let _ = tx.send(WorkerMsg::Done);
                                return;
                            };
                            last_output = std::time::Instant::now();
                            let bytes = match chunk {
                                Ok(b) => b,
                                Err(e) => {
                                    let _ = tx.send(WorkerMsg::Error(format!("Stream read failed: {e}")));
                                    return;
                                }
                            };
                            buf.push_str(&String::from_utf8_lossy(bytes.as_ref()));
                            while let Some(pos) = buf.find('\n') {
                                let line: String = buf.drain(..=pos).collect();
                                let line = line.trim();
                                if line.is_empty() || line.starts_with(':') {
                                    continue;
                                }
                                if let Some(ev) = parse_sse_line(line) {
                                    if tx.send(WorkerMsg::Event(ev)).is_err() {
                                        return; // consumer gone
                                    }
                                }
                            }
                        }
                        _ = ticker.tick() => {
                            if worker_gone.load(std::sync::atomic::Ordering::SeqCst) {
                                return;
                            }
                            if last_output.elapsed() > STALL_TIMEOUT {
                                let _ = tx.send(WorkerMsg::Error(format!(
                                    "server stalled — no output for {}s; check the server logs (VRAM pressure?)",
                                    STALL_TIMEOUT.as_secs()
                                )));
                                return;
                            }
                        }
                    }
                }
            });
        });

        let mut finish = String::from("stop");
        // Persistent ticker: a fresh `sleep` per iteration would be reset by
        // every token, so Stop would never be observed mid-stream.
        let mut stop_tick = tokio::time::interval(Duration::from_millis(200));
        stop_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    None | Some(WorkerMsg::Done) => break,
                    Some(WorkerMsg::Event(ev)) => match ev {
                        StreamEvent::Finish { reason } => {
                            finish = reason.clone();
                            on_event(StreamEvent::Finish { reason });
                        }
                        other => on_event(other),
                    },
                    Some(WorkerMsg::Error(e)) => {
                        gone.store(true, std::sync::atomic::Ordering::SeqCst);
                        bail!("{e}");
                    }
                },
                _ = stop_tick.tick() => {
                    if should_stop() {
                        // Flag the worker so an idle stream exits too, then
                        // return: the dropped runtime closes the connection.
                        gone.store(true, std::sync::atomic::Ordering::SeqCst);
                        bail!("aborted");
                    }
                }
            }
        }
        Ok(finish)
    }

    /// Largest `(n_ctx, prompt + generated)` across `/slots`.
    pub async fn slot_fill(&self) -> Result<Option<(u64, u64)>> {
        let resp = self
            .authed(self.http.get(format!("{}/slots", self.base_url)))
            .send()
            .await
            .context("Slots request failed")?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        Ok(max_slot_fill(&resp.text().await?))
    }

    /// Runtime context size from `/props` (server config, not training max).
    pub async fn props_context(&self) -> Result<Option<u64>> {
        let resp = self
            .authed(self.http.get(format!("{}/props", self.base_url)))
            .send()
            .await
            .context("Props request failed")?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        Ok(parse_props_n_ctx(&resp.text().await?))
    }

    /// Bearer auth when the server was launched with an API key.
    fn authed(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }
}

pub fn max_slot_n_ctx(json_text: &str) -> Option<u64> {
    let v: Value = serde_json::from_str(json_text).ok()?;
    v.as_array()?
        .iter()
        .filter_map(|s| s.get("n_ctx")?.as_u64())
        .max()
}

/// `default_generation_settings.n_ctx` from `/props`.
pub fn parse_props_n_ctx(json_text: &str) -> Option<u64> {
    let v: Value = serde_json::from_str(json_text).ok()?;
    v.get("default_generation_settings")?
        .get("n_ctx")?
        .as_u64()
        .filter(|n| *n > 0)
}

pub fn max_slot_fill(json_text: &str) -> Option<(u64, u64)> {
    let v: Value = serde_json::from_str(json_text).ok()?;
    let arr = v.as_array()?;
    let ctx = arr.iter().filter_map(|s| s.get("n_ctx")?.as_u64()).max()?;
    let used = arr
        .iter()
        .map(|s| {
            let num = |v: &Value, k: &str| v.get(k).and_then(|n| n.as_u64()).unwrap_or(0);
            // Schemas vary: `n_prompt`/`n_predicted` (old) vs
            // `n_prompt_tokens`/`next_token.n_decoded` (new). Take the larger
            // reading so nothing double-counts.
            let prompt = num(s, "n_prompt_tokens").max(num(s, "n_prompt"));
            let decoded = s
                .get("next_token")
                .map(|nt| num(nt, "n_decoded").max(num(nt, "n_predicted")))
                .unwrap_or(0);
            (prompt + decoded.max(num(s, "n_predicted"))).max(num(s, "n_tokens"))
        })
        .max()
        .unwrap_or(0);
    Some((ctx, used))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_chunk_without_choices_parses() {
        let ev = parse_sse_line(
            r#"data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#,
        )
        .unwrap();
        assert_eq!(ev, StreamEvent::Usage { prompt_tokens: 12, completion_tokens: 3 });
    }

    #[test]
    fn provider_errors_surface_as_notices() {
        let top = parse_sse_line(r#"data: {"error":{"code":502,"message":"upstream down"}}"#)
            .unwrap();
        match top {
            StreamEvent::Notice { text } => {
                assert!(text.contains("upstream down") && text.contains("502"), "{text}")
            }
            other => panic!("wrong event: {other:?}"),
        }
        let choice =
            parse_sse_line(r#"data: {"choices":[{"error":{"code":429,"message":"rate limited"}}]}"#)
                .unwrap();
        match choice {
            StreamEvent::Notice { text } => assert!(text.contains("rate limited"), "{text}"),
            other => panic!("wrong event: {other:?}"),
        }
    }

    /// Stop must end the stream promptly, and dropping the per-run client
    /// (as the run teardown does) must close the connection — OpenRouter
    /// cancels upstream generation when the connection drops.
    #[tokio::test]
    async fn stop_flag_aborts_stream_and_closes_connection() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicBool, Ordering};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
            );
            let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
            let frame = format!("{:x}\r\n{}\r\n", body.len(), String::from_utf8_lossy(body));
            loop {
                // A closed peer reads as 0; a reset surfaces as an error.
                match sock.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(true);
                        return;
                    }
                    Ok(_) => {}
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => {
                        let _ = tx.send(true);
                        return;
                    }
                }
                if sock.write_all(frame.as_bytes()).is_err() {
                    let _ = tx.send(true);
                    return;
                }
            }
        });

        let client = LlmClient::new(format!("http://{addr}"));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stop2.store(true, Ordering::SeqCst);
        });
        let started = std::time::Instant::now();
        let err = client
            .chat_stream(
                None,
                &[ChatMessage::user("hi")],
                None,
                None,
                None,
                || stop.load(Ordering::SeqCst),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("aborted"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3));
        // Aborting tears down the worker runtime, which closes the
        // connection; the mock server observes the disconnect.
        assert!(rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false));
    }

    #[test]
    fn base_urls_and_error_bodies_normalize() {
        assert_eq!(normalize_base("https://openrouter.ai/api/v1/"), "https://openrouter.ai/api");
        assert_eq!(normalize_base("https://api.deepseek.com"), "https://api.deepseek.com");
        assert_eq!(normalize_base(" http://localhost:1234/v1 "), "http://localhost:1234");
        let detail =
            http_error_detail(401, r#"{"error":{"code":401,"message":"No auth credentials found"}}"#);
        assert!(detail.contains("No auth credentials found") && detail.contains("401"), "{detail}");
        // OpenRouter forwards upstream failures in metadata.raw.
        let limited = http_error_detail(
            429,
            r#"{"error":{"code":429,"message":"Provider returned error","metadata":{"raw":"Rate limit exceeded: free-models-per-day. Add 10 credits to unlock 1000 free model requests per day","provider_name":"Venice"}}}"#,
        );
        assert!(
            limited.contains("free-models-per-day")
                && limited.contains("Provider returned error")
                && limited.contains("Venice"),
            "{limited}"
        );
        let raw = http_error_detail(500, "boom");
        assert!(raw.contains("500") && raw.contains("boom"), "{raw}");
    }

    #[test]
    fn sse_content_and_tool_deltas_parse() {
        let content = parse_sse_line(
            r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#,
        )
        .unwrap();
        assert_eq!(content, StreamEvent::Content { text: "hi".into() });
        let tool = parse_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#,
        )
        .unwrap();
        match tool {
            StreamEvent::ToolCallDelta { index, id, name, arguments_delta } => {
                assert_eq!((index, id.as_deref(), name.as_deref()), (0, Some("c1"), Some("read_file")));
                assert!(arguments_delta.contains("path"));
            }
            other => panic!("wrong event: {other:?}"),
        }
        assert!(parse_sse_line("data: [DONE]").is_some());
        assert!(parse_sse_line(": keep-alive").is_none());
    }

    #[test]
    fn collector_assembles_split_call() {
        let mut col = StreamCollector::default();
        col.push(&StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            name: Some("read".into()),
            arguments_delta: "{\"path\": \"a".into(),
        });
        col.push(&StreamEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments_delta: ".rs\"}".into(),
        });
        let calls = col.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "read");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["path"], Value::String("a.rs".into()));
    }

    #[test]
    fn strips_leaked_chat_control_tokens() {
        assert_eq!(strip_special_tokens("Hello<|im_end|>"), "Hello");
        assert_eq!(
            strip_special_tokens("A<｜end▁of▁sentence｜> B"),
            "A B"
        );
        assert_eq!(strip_special_tokens("<s>Hello</s>"), "Hello");
        assert_eq!(
            strip_special_tokens("the <s> and </s> tokens matter"),
            "the <s> and </s> tokens matter"
        );
    }

    #[test]
    fn slot_fill_takes_largest_slot() {
        let text = r#"[{"n_ctx":8192,"n_prompt":100,"n_predicted":50},{"n_ctx":32768,"n_prompt_tokens":200,"next_token":{"n_decoded":30}}]"#;
        assert_eq!(max_slot_fill(text), Some((32768, 230)));
        assert_eq!(max_slot_fill("[]"), None);
        assert_eq!(max_slot_fill("garbage"), None);
    }

    #[test]
    fn props_n_ctx_reads_generation_settings() {
        let text = r#"{"default_generation_settings":{"n_ctx":16384}}"#;
        assert_eq!(parse_props_n_ctx(text), Some(16384));
        assert_eq!(parse_props_n_ctx("{}"), None);
    }
}