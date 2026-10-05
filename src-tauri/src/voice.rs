//! Voice: text-to-speech through llama.cpp's `llama-tts` tool (Qwen3-TTS).

use serde::Serialize;

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct TtsResult {
    pub path: String,
    /// Base64 WAV so the webview can play it without asset-protocol setup.
    pub audio: String,
}

/// Synthesize `text` with the configured TTS model.
pub async fn synthesize(state: &crate::AppState, text: &str) -> Result<TtsResult, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Nothing to speak".to_string());
    }
    let app_config = state.config.lock().unwrap().clone();
    let model = app_config
        .assistant
        .tts_model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| "Pick a TTS model on the Assistant Voice tab".to_string())?
        .to_string();
    if !std::path::Path::new(&model).is_file() {
        return Err(format!("TTS model not found: {model}"));
    }
    let base = crate::runtime::runtimes_base_dir().map_err(|e| e.to_string())?;
    let exe = crate::runtime::sibling_binary(&app_config, &base, "llama-tts")
        .map_err(|e| format!("llama-tts is not in this runtime build: {e}"))?;
    let dir = crate::system::temp_workspace().join("voice");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let out = dir.join(format!("tts-{millis}.wav"));

    let mmproj = app_config
        .assistant
        .tts_mmproj
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let speaker = app_config
        .assistant
        .tts_speaker
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let lang = app_config.assistant.tts_lang.trim();

    // GPU first; a CUDA failure (some TTS graphs hit it) retries on the CPU.
    let first = run_tts(&exe, &model, mmproj, speaker, lang, text, &out, 999).await?;
    let output = if first.status.success() || !tts_is_cuda_error(&first.stderr) {
        first
    } else {
        let _ = std::fs::remove_file(&out);
        run_tts(&exe, &model, mmproj, speaker, lang, text, &out, 0).await?
    };
    if !output.status.success() {
        return Err(format!(
            "llama-tts exited with {}: {}",
            output.status,
            tts_error_tail(&output.stderr)
        ));
    }
    let bytes = std::fs::read(&out).map_err(|e| format!("llama-tts produced no audio: {e}"))?;
    Ok(TtsResult {
        path: out.to_string_lossy().to_string(),
        audio: crate::roleplay::encode_base64(&bytes),
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_tts(
    exe: &std::path::Path,
    model: &str,
    mmproj: Option<&str>,
    speaker: Option<&str>,
    lang: &str,
    text: &str,
    out: &std::path::Path,
    ngl: u32,
) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new(exe);
    crate::hidden::hide_tokio(&mut cmd);
    cmd.kill_on_drop(true)
        .arg("-m")
        .arg(model)
        .arg("-p")
        .arg(text)
        .arg("--tts-lang")
        .arg(lang)
        .arg("-n")
        .arg("2048")
        .arg("-ngl")
        .arg(ngl.to_string())
        .arg("--output")
        .arg(out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    if let Some(mmproj) = mmproj {
        cmd.arg("-mm").arg(mmproj);
    }
    if let Some(speaker) = speaker {
        cmd.arg("--tts-speaker-file").arg(speaker);
    }
    tokio::time::timeout(std::time::Duration::from_secs(180), cmd.output())
        .await
        .map_err(|_| "llama-tts timed out".to_string())?
        .map_err(|e| format!("Cannot run llama-tts: {e}"))
}

/// A CUDA abort inside llama-tts: retry on the CPU instead of surfacing it.
fn tts_is_cuda_error(stderr: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(stderr).to_lowercase();
    lower.contains("cuda error") || lower.contains("ggml-cuda")
}

/// llama-tts streams a progress meter on stderr; surface real errors first,
/// otherwise the last few meaningful lines.
fn tts_error_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let is_error = |l: &str| {
        let lower = l.to_lowercase();
        [
            "error",
            "failed",
            "unknown",
            "invalid",
            "not found",
            "cannot",
            "unable",
            "requires",
        ]
        .iter()
        .any(|k| lower.contains(k))
    };
    let errors: Vec<&str> = lines.iter().rev().filter(|l| is_error(l)).take(3).copied().collect();
    let picked: Vec<&str> = if errors.is_empty() {
        lines.iter().rev().take(5).copied().collect()
    } else {
        errors
    };
    let tail = picked.into_iter().rev().collect::<Vec<_>>().join(" | ");
    if tail.is_empty() {
        "no output".to_string()
    } else {
        tail
    }
}

/// Speak `text` with the configured voice; returns the WAV path and bytes.
#[tauri::command]
#[specta::specta]
pub async fn assistant_tts_speak(
    text: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<TtsResult, String> {
    synthesize(&state, &text).await
}
