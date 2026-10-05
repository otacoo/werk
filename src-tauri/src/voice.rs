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

    let mut cmd = tokio::process::Command::new(&exe);
    crate::hidden::hide_tokio(&mut cmd);
    cmd.kill_on_drop(true)
        .arg("-m")
        .arg(&model)
        .arg("-p")
        .arg(text)
        .arg("--tts-lang")
        .arg(app_config.assistant.tts_lang.trim())
        .arg("-n")
        .arg("2048")
        .arg("-ngl")
        .arg("999")
        .arg("--output")
        .arg(&out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    if let Some(speaker) = app_config
        .assistant
        .tts_speaker
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.arg("--tts-speaker-file").arg(speaker);
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(180), cmd.output())
        .await
        .map_err(|_| "llama-tts timed out".to_string())?
        .map_err(|e| format!("Cannot run llama-tts: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let tail: String = err
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(format!("llama-tts failed: {}", tail.trim()));
    }
    let bytes = std::fs::read(&out).map_err(|e| format!("llama-tts produced no audio: {e}"))?;
    Ok(TtsResult {
        path: out.to_string_lossy().to_string(),
        audio: crate::roleplay::encode_base64(&bytes),
    })
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
