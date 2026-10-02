//! Context compaction: oldest turns fold into a model-written summary.
//! Cuts land on user boundaries; standing user lines ride verbatim.

use anyhow::{bail, Result};
use serde_json::Value;

use crate::agent::AgentEvent;
use crate::client::{ChatMessage, LlmClient, StreamEvent};

/// Recent messages kept verbatim, scaled to the window: a fixed tail fits
/// a large window but starves a small one, so keep a quarter clamped.
pub const KEEP_RECENT_TOKENS: u64 = 8_192;
pub fn keep_budget(context_limit: u64) -> u64 {
    KEEP_RECENT_TOKENS.min((context_limit / 4).max(1_024))
}
/// Tokens reserved for the reply + tool schemas; the trigger is
/// `estimate > limit - reserve`, capped for tiny windows.
pub fn reserve_tokens(context_limit: u64) -> u64 {
    16_384.min(context_limit / 2)
}
/// Tool results above this many chars are trimmed head/tail in place —
/// often relieves pressure without any summary call.
pub const TOOL_RESULT_TRIM_THRESHOLD: usize = 8_192;
/// How far back to scan for a user boundary before giving up.
const MAX_SCAN: usize = 48;
/// Total transcript text fed to the summarizer; very long sessions keep the
/// newest content (older text is dropped with a marker) since chunked folding
/// bounds each call, and this only bounds total cost.
const SUMMARIZE_TOTAL_CAP_CHARS: usize = 200_000;
/// Per-call transcript chunk (~5k tokens). Chunked folding keeps every call
/// inside a small utility-model window.
const SUMMARY_CHUNK_CHARS: usize = 20_000;
/// Per-message cap inside the summarized transcript.
const MESSAGE_CAP_CHARS: usize = 2_000;
/// Verbatim user-message budget inside the summary block (~2k tokens).
const VERBATIM_USER_CHARS: usize = 8_000;
/// Summary marker; also the carry marker for verbatim folds.
pub const SUMMARY_MARKER: &str = "[Compacted context";
const VERBATIM_MARKER: &str = "[Standing user messages — verbatim]";

/// Chars/4 estimate. Array content counts text parts only — raw JSON would
/// charge every attached image's base64 payload to the transcript.
pub fn estimate_tokens(history: &[ChatMessage]) -> u64 {
    let chars: usize = history
        .iter()
        .filter_map(|m| m.content.as_ref())
        .map(|c| match c {
            Value::String(s) => s.len(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| {
                    if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                        p.get("text").and_then(|t| t.as_str()).map(|s| s.len())
                    } else {
                        None
                    }
                })
                .sum::<usize>(),
            other => other.to_string().len(),
        })
        .sum();
    (chars / 4) as u64
}

/// Token cost of the tool schemas: sent on every request but absent from
/// `history`, so compaction must subtract them from the window.
pub fn schema_tokens(schemas: &[Value]) -> u64 {
    (schemas.iter().map(|s| s.to_string().len()).sum::<usize>() / 4) as u64
}

pub fn is_summary(m: &ChatMessage) -> bool {
    m.content
        .as_ref()
        .and_then(|c| c.as_str())
        .map(|s| s.starts_with(SUMMARY_MARKER))
        .unwrap_or(false)
}

/// Cut index (first kept message) honoring user boundaries, keeping the
/// newest messages worth ~`keep_tokens`. None when nothing is removable.
pub fn plan_cut(history: &[ChatMessage], keep_tokens: u64) -> Option<usize> {
    let protected = if history.first().map(|m| m.role.as_str()) == Some("system") {
        1
    } else {
        0
    };
    if history.len() <= protected + 3 {
        return None;
    }
    let mut cut = protected;
    let mut acc = 0u64;
    for idx in (protected..history.len()).rev() {
        cut = idx;
        acc += estimate_tokens(&history[idx..idx + 1]);
        if acc >= keep_tokens {
            break;
        }
    }
    let floor = cut.saturating_sub(MAX_SCAN).max(protected);
    for c in (floor..=cut).rev() {
        if history[c].role == "user" && !is_summary(&history[c]) {
            return Some(c);
        }
    }
    // No user turn in range: advance past non-users and old summaries.
    let mut c = cut;
    while c < history.len() && (history[c].role != "user" || is_summary(&history[c])) {
        c += 1;
    }
    (c < history.len()).then_some(c)
}

/// Trim over-budget tool results in place; returns the trimmed count.
pub fn prune_tool_results(history: &mut [ChatMessage]) -> usize {
    let mut trimmed = 0;
    for m in history.iter_mut() {
        if m.role != "tool" {
            continue;
        }
        let Some(Value::String(text)) = m.content.as_ref() else {
            continue;
        };
        if text.chars().count() <= TOOL_RESULT_TRIM_THRESHOLD {
            continue;
        }
        let head: String = text.chars().take(TOOL_RESULT_TRIM_THRESHOLD / 2).collect();
        let tail: String = text.chars().skip(text.chars().count() - 1_024).collect();
        m.content = Some(Value::String(format!(
            "{head}\n[…middle pruned to save context; the session log has the full output…]\n{tail}"
        )));
        trimmed += 1;
    }
    trimmed
}

fn message_text(m: &ChatMessage) -> String {
    match m.content.as_ref() {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                    p.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
    }
}

fn render_for_summary(m: &ChatMessage) -> Option<String> {
    let mut text = message_text(m);
    if text.chars().count() > MESSAGE_CAP_CHARS {
        text = text.chars().take(MESSAGE_CAP_CHARS).collect();
        text.push_str("\n[…]");
    }
    if text.trim().is_empty() && m.tool_calls.as_ref().map(|c| c.is_empty()).unwrap_or(true) {
        return None;
    }
    let line = match m.role.as_str() {
        "user" => format!("user: {text}"),
        "assistant" => match m.tool_calls.as_ref().filter(|c| !c.is_empty()) {
            Some(calls) => {
                let names: Vec<String> = calls
                    .iter()
                    .map(|c| {
                        let args = c.function.arguments.chars().take(120).collect::<String>();
                        format!("{}({})", c.function.name, args)
                    })
                    .collect();
                if text.trim().is_empty() {
                    format!("assistant tool calls: {}", names.join(", "))
                } else {
                    format!("assistant: {text} [tool calls: {}]", names.join(", "))
                }
            }
            None => format!("assistant: {text}"),
        },
        "tool" => format!("tool result: {text}"),
        _ => return None,
    };
    Some(line)
}

/// Newest-first verbatim user blocks, capped to budget, returned
/// chronologically. Paraphrase softens stated constraints, so standing
/// instructions ride quoted; a previous fold's block carries forward.
fn verbatim_user_section(span: &[ChatMessage]) -> Option<String> {
    let mut blocks: Vec<String> = Vec::new();
    for m in span.iter().rev() {
        if m.role.as_str() != "user" {
            continue;
        }
        let text = message_text(m);
        if text.trim().is_empty() {
            continue;
        }
        if is_summary(m) {
            if let Some(idx) = text.find(VERBATIM_MARKER) {
                for line in text[idx + VERBATIM_MARKER.len()..].lines().rev() {
                    let line = line.trim();
                    if !line.is_empty() {
                        blocks.push(line.to_string());
                    }
                }
            }
            continue;
        }
        let mut t = text.trim().to_string();
        if t.chars().count() > MESSAGE_CAP_CHARS {
            t = t.chars().take(MESSAGE_CAP_CHARS).collect();
            t.push_str("\n[…]");
        }
        blocks.push(t);
    }
    if blocks.is_empty() {
        return None;
    }
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0usize;
    for b in blocks {
        let n = b.chars().count() + 1;
        if used + n > VERBATIM_USER_CHARS && !kept.is_empty() {
            break;
        }
        used += n;
        kept.push(b);
    }
    kept.reverse();
    Some(format!("{VERBATIM_MARKER}\n{}", kept.join("\n")))
}

/// Cumulative file operations: every write/edit path in the span, so the
/// summary states the final file state.
fn file_ops_section(history: &[ChatMessage]) -> Option<String> {
    let mut paths: Vec<String> = Vec::new();
    for m in history {
        let Some(calls) = m.tool_calls.as_ref().filter(|c| !c.is_empty()) else {
            continue;
        };
        for call in calls {
            if !matches!(call.function.name.as_str(), "write_file" | "edit_file") {
                continue;
            }
            if let Ok(args) = serde_json::from_str::<Value>(&call.function.arguments) {
                if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
                    let p = p.trim();
                    if !p.is_empty() && !paths.iter().any(|x| x == p) {
                        paths.push(p.to_string());
                    }
                }
            }
        }
    }
    if paths.is_empty() {
        return None;
    }
    Some(format!(
        "Files created or modified during this conversation:\n{}",
        paths.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")
    ))
}

pub struct CompactionInfo {
    /// First kept index (for meta reindexing). None when only tool
    /// results were pruned (no summary, nothing reindexed).
    pub cut: Option<usize>,
    pub removed: usize,
}

fn over_pressure(history: &[ChatMessage], context_limit: u64) -> bool {
    estimate_tokens(history) > context_limit.saturating_sub(reserve_tokens(context_limit))
}

const COMPACT_SUMMARY_SYSTEM: &str =
    "Summarize this conversation for continuation in a fresh context window. \
    Preserve: the user's goals, key decisions and why, files created or changed, \
    pending tasks, and any durable facts or preferences. Be concise; use bullets.";

/// Render messages for summarization (file ops appended, total capped).
pub fn render_transcript(history: &[ChatMessage]) -> String {
    let rendered: Vec<String> = history.iter().filter_map(render_for_summary).collect();
    let files = file_ops_section(history);
    let mut transcript = rendered.join("\n\n");
    let chars = transcript.chars().count();
    if chars > SUMMARIZE_TOTAL_CAP_CHARS {
        // Keep the newest turns: continuity matters more than deep history.
        let tail: String = transcript.chars().skip(chars - SUMMARIZE_TOTAL_CAP_CHARS).collect();
        transcript = format!("[…earlier messages omitted to bound the summary cost…]\n\n{tail}");
    }
    if let Some(files) = files {
        transcript.push_str("\n\n");
        transcript.push_str(&files);
    }
    transcript
}

/// Split a transcript into chunks of at most `chunk_chars`, preferring line
/// boundaries; a single oversized line is split by characters.
pub fn split_transcript(text: &str, chunk_chars: usize) -> Vec<String> {
    let cap = chunk_chars.max(1);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for line in text.split_inclusive('\n') {
        let line_len = line.chars().count();
        if line_len > cap {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                current_len = 0;
            }
            let mut piece = String::new();
            let mut n = 0usize;
            for ch in line.chars() {
                piece.push(ch);
                n += 1;
                if n == cap {
                    chunks.push(std::mem::take(&mut piece));
                    n = 0;
                }
            }
            if !piece.is_empty() {
                chunks.push(piece);
            }
            continue;
        }
        if current_len + line_len > cap {
            chunks.push(std::mem::take(&mut current));
            current_len = 0;
        }
        current.push_str(line);
        current_len += line_len;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// One-shot summary turn: no tools, streams content into `on_text`.
pub async fn summarize(
    client: &LlmClient,
    model: Option<&str>,
    system: &str,
    transcript: &str,
    should_stop: &(dyn Fn() -> bool + Send + Sync),
    mut on_text: impl FnMut(&str),
) -> Result<String> {
    let mut summary = String::new();
    client
        .chat_stream(
            model,
            &[ChatMessage::system(system), ChatMessage::user(transcript)],
            None,
            None,
            None,
            || should_stop(),
            &mut |ev| {
                if let StreamEvent::Content { text } = ev {
                    summary.push_str(&text);
                    on_text(&text);
                }
            },
        )
        .await?;
    if should_stop() {
        bail!("aborted");
    }
    Ok(summary.trim().to_string())
}

/// Fold a long transcript in bounded chunks: each call sees the running
/// summary plus one chunk, so the summarizer only needs a small window.
pub async fn summarize_chunked(
    client: &LlmClient,
    model: Option<&str>,
    system: &str,
    transcript: &str,
    should_stop: &(dyn Fn() -> bool + Send + Sync),
    mut on_text: impl FnMut(&str),
) -> Result<String> {
    let chunks = split_transcript(transcript, SUMMARY_CHUNK_CHARS);
    if chunks.is_empty() {
        bail!("Nothing to summarize");
    }
    let mut running = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let input = if running.is_empty() {
            chunk.clone()
        } else {
            format!(
                "Summary so far:\n{running}\n\nNext transcript segment ({} of {}):\n{chunk}",
                i + 1,
                chunks.len()
            )
        };
        running = summarize(client, model, system, &input, should_stop, &mut on_text).await?;
        if running.is_empty() {
            bail!("Summarizer returned an empty summary");
        }
    }
    Ok(running)
}

/// Summarize `history[1..cut]` and splice in one summary message. None when
/// under the trigger (unless `force`); errors when compaction cannot help.
pub async fn compact_history(
    client: &LlmClient,
    model: Option<&str>,
    history: &mut Vec<ChatMessage>,
    context_limit: u64,
    force: bool,
    should_stop: &(dyn Fn() -> bool + Send + Sync),
    on_event: &mut (dyn FnMut(AgentEvent) + Send),
) -> Result<Option<CompactionInfo>> {
    if context_limit == 0 || (!force && !over_pressure(history, context_limit)) {
        return Ok(None);
    }
    let pruned = prune_tool_results(history);
    if pruned > 0 && (force || !over_pressure(history, context_limit)) {
        on_event(AgentEvent::Notice {
            text: format!("Trimmed {pruned} oversized tool result(s) to save context."),
        });
        if !force {
            return Ok(Some(CompactionInfo { cut: None, removed: pruned }));
        }
    }
    let keep = keep_budget(context_limit);
    let Some(cut) = plan_cut(history, keep) else {
        bail!(
            "Conversation is too long to continue and has no compactable turns \
            ({} tokens vs {} limit) — start a new chat (/new)",
            estimate_tokens(history),
            context_limit
        );
    };
    let tail_estimate = estimate_tokens(&history[cut..]);
    if tail_estimate > (context_limit as f64 * 0.9) as u64 {
        bail!(
            "Conversation is too long even after compaction (kept tail {} tokens vs {} limit) \
            — start a new chat (/new)",
            tail_estimate,
            context_limit
        );
    }
    let transcript = render_transcript(&history[1..cut]);
    if transcript.trim().is_empty() {
        bail!(
            "Conversation is too long even after compaction (nothing summarizable in {} tokens) \
            — start a new chat (/new)",
            estimate_tokens(history)
        );
    }
    let summary = summarize_chunked(
        client,
        model,
        COMPACT_SUMMARY_SYSTEM,
        &transcript,
        should_stop,
        |_| {},
    )
    .await?;
    if summary.is_empty() {
        bail!("Compaction produced an empty summary — start a new chat (/new)");
    }
    let removed = cut - 1;
    let mut summary_msg = format!(
        "{SUMMARY_MARKER} — summary of {removed} earlier messages]\n\n{summary}"
    );
    if let Some(verbatim) = verbatim_user_section(&history[1..cut]) {
        summary_msg.push_str("\n\n");
        summary_msg.push_str(&verbatim);
    }
    history.splice(1..cut, [ChatMessage::user(summary_msg)]);
    on_event(AgentEvent::Compacted { removed });
    Ok(Some(CompactionInfo { cut: Some(cut), removed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_splits_into_bounded_chunks() {
        let text = format!("{}\n{}\n{}", "a".repeat(50), "b".repeat(30_000), "c".repeat(10));
        let chunks = split_transcript(&text, 1_000);
        assert!(chunks.len() >= 31, "{}", chunks.len());
        for c in &chunks {
            assert!(c.chars().count() <= 1_000, "chunk of {}", c.chars().count());
        }
        assert_eq!(chunks.join(""), text);
        assert!(split_transcript("", 100).is_empty());
    }

    #[test]
    fn long_transcripts_keep_the_newest_content() {
        let mut h = Vec::new();
        for i in 0..120 {
            h.push(user(&format!("msg {i} {}", "x".repeat(1_900))));
        }
        h.push(user("the newest instruction"));
        let text = render_transcript(&h);
        assert!(text.contains("earlier messages omitted"), "{}", &text[..120]);
        assert!(text.contains("the newest instruction"));
        assert!(!text.contains("msg 0 "), "oldest content should be dropped");
    }

    #[test]
    fn schema_overhead_counts_outside_history() {
        let schemas = vec![
            serde_json::json!({"type": "function", "function": {"name": "read_file", "parameters": {"type": "object"}}}),
            serde_json::json!({"type": "function", "function": {"name": "write_file", "parameters": {"type": "object"}}}),
        ];
        let tokens = schema_tokens(&schemas);
        assert!(tokens > 0);
        assert!(schema_tokens(&[]).eq(&0));
        // A ~10k-token window can't hold a large schema block plus history.
        let limit = 10_240u64;
        assert!(keep_budget(limit.saturating_sub(tokens * 6)) < keep_budget(limit));
    }

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user(text.to_string())
    }

    fn history(n_turns: usize) -> Vec<ChatMessage> {
        let mut h = vec![ChatMessage::system("sys")];
        for i in 0..n_turns {
            h.push(user(&format!("question {i}")));
            h.push(ChatMessage::assistant(format!("answer {i}")));
        }
        h
    }

    #[test]
    fn estimate_counts_chars_over_four() {
        let h = vec![ChatMessage::system("12345678")];
        assert_eq!(estimate_tokens(&h), 2);
        assert_eq!(estimate_tokens(&[]), 0);
    }

    #[test]
    fn plan_cut_keeps_budget_on_user_boundary() {
        let mut h = vec![ChatMessage::system("sys")];
        for i in 0..20 {
            h.push(user(&format!("question {i}")));
            h.push(ChatMessage::assistant("x".repeat(2000)));
        }
        let cut = plan_cut(&h, 4_096).unwrap();
        assert_eq!(h[cut].role, "user");
        let tail = estimate_tokens(&h[cut..]);
        assert!(tail >= 4_096, "budget reached: {tail}");
        assert!(tail <= 4_096 + 600, "not overshot by more than a message: {tail}");
    }

    #[test]
    fn plan_cut_nothing_to_remove() {
        assert_eq!(plan_cut(&history(1), 4_096), None);
    }

    #[test]
    fn keep_budget_scales_to_small_windows() {
        assert_eq!(keep_budget(40_000), 8_192);
        assert_eq!(keep_budget(8_000), 2_000);
        assert_eq!(keep_budget(4_096), 1_024);
    }

    #[test]
    fn verbatim_section_keeps_user_lines_chronological() {
        let h = vec![
            ChatMessage::system("sys"),
            user("use ESM not CJS"),
            ChatMessage::assistant("ok"),
            user("tabs, no spaces"),
        ];
        let section = verbatim_user_section(&h[1..]).unwrap();
        assert!(section.starts_with(VERBATIM_MARKER));
        let a = section.find("ESM").unwrap();
        let b = section.find("tabs").unwrap();
        assert!(a < b);
        assert!(!section.contains("noted") && !section.contains("ok\n"));
    }

    #[test]
    fn verbatim_section_carries_previous_fold() {
        let old = format!("{VERBATIM_MARKER}\nuse ESM not CJS");
        let h = vec![
            ChatMessage::system("sys"),
            ChatMessage::user(format!(
                "{SUMMARY_MARKER} — summary of 2 earlier messages]\n\nold prose\n\n{old}"
            )),
            user("also: no semicolons"),
        ];
        let section = verbatim_user_section(&h[1..]).unwrap();
        assert!(section.contains("use ESM not CJS"));
        assert!(section.contains("no semicolons"));
        assert!(!section.contains("old prose"));
    }

    #[test]
    fn prune_tool_results_trims_over_budget() {
        let big = "y".repeat(TOOL_RESULT_TRIM_THRESHOLD * 2);
        let mut h = vec![
            ChatMessage::user("q"),
            ChatMessage {
                role: "tool".into(),
                content: Some(Value::String(big.clone())),
                tool_calls: None,
                tool_call_id: Some("c".into()),
            },
        ];
        assert_eq!(prune_tool_results(&mut h), 1);
        let content = h[1].content.as_ref().and_then(|c| c.as_str()).unwrap();
        assert!(content.contains("middle pruned"));
    }

    #[test]
    fn small_window_tail_fits_backstop() {
        for limit in [4_096u64, 8_000, 16_000] {
            let mut h = vec![ChatMessage::system("sys")];
            for i in 0..30 {
                h.push(user(&format!("question {i}")));
                h.push(ChatMessage::assistant("x".repeat(1500)));
            }
            while estimate_tokens(&h) <= limit.saturating_sub(reserve_tokens(limit)) {
                let i = h.len();
                h.push(user(&format!("extra {i}")));
                h.push(ChatMessage::assistant("y".repeat(1500)));
            }
            let cut = plan_cut(&h, keep_budget(limit)).expect("must find a cut");
            let tail = estimate_tokens(&h[cut..]);
            assert!(tail <= (limit as f64 * 0.9) as u64, "limit {limit}: tail {tail}");
            assert!(cut > 1);
        }
    }
}
