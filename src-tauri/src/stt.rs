//! Speech-to-text for the overlay microphone: one-shot llama-mtmd-cli runs
//! against a Qwen3-ASR GGUF (model + audio mmproj).

use tauri::State;

/// Transcription result: the text plus the model's detected language.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct SttResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Qwen3-ASR answers as `language English<asr_text>the words…`; keep the
/// text after the marker and the language before it.
pub(crate) fn parse_asr_output(raw: &str) -> SttResult {
    let raw = raw.trim();
    let (language, text) = match raw.split_once("<asr_text>") {
        Some((head, text)) => (parse_language(head), text.trim().to_string()),
        None => (None, raw.to_string()),
    };
    SttResult { text, language }
}

fn parse_language(head: &str) -> Option<String> {
    let head = head.trim();
    head.strip_prefix("language")
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
}

/// Transcribe a base64 WAV (16 kHz mono PCM16) recorded in the webview.
#[tauri::command]
#[specta::specta]
pub async fn assistant_stt_transcribe(
    audio: String,
    state: State<'_, crate::AppState>,
) -> Result<SttResult, String> {
    let app_config = state.config.lock().unwrap().clone();
    let model = app_config
        .assistant
        .stt_model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| "Pick an STT model on the Assistant Voice tab".to_string())?
        .to_string();
    if !std::path::Path::new(&model).is_file() {
        return Err(format!("STT model not found: {model}"));
    }
    let mmproj = app_config
        .assistant
        .stt_mmproj
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Pick the STT audio projector (mmproj) on the Voice tab".to_string())?;
    if !std::path::Path::new(&mmproj).is_file() {
        return Err(format!("STT mmproj not found: {mmproj}"));
    }
    let base = crate::runtime::runtimes_base_dir().map_err(|e| e.to_string())?;
    let exe = crate::runtime::sibling_binary(&app_config, &base, "llama-mtmd-cli")
        .map_err(|e| format!("llama-mtmd-cli is not in this runtime build: {e}"))?;
    let bytes = crate::roleplay::decode_base64(audio.trim())?;
    if bytes.is_empty() {
        return Err("No audio recorded".to_string());
    }
    let dir = crate::system::temp_workspace().join("voice");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let wav = dir.join(format!("stt-{millis}.wav"));
    std::fs::write(&wav, &bytes).map_err(|e| e.to_string())?;

    // A forced language prefills the transcript language the way the official
    // Qwen3-ASR SDK does; empty keeps automatic detection.
    let lang = app_config.assistant.stt_lang.trim().to_string();
    let prompt = if lang.is_empty() {
        "Transcribe the audio.".to_string()
    } else {
        format!("language {lang}<asr_text>")
    };

    // GPU first; a CUDA failure retries on the CPU, like llama-tts.
    let first = run_stt(&exe, &model, &mmproj, &wav, &prompt, 999).await?;
    let output = if first.status.success() || !crate::voice::is_cuda_error(&first.stderr) {
        first
    } else {
        run_stt(&exe, &model, &mmproj, &wav, &prompt, 0).await?
    };
    if !output.status.success() {
        let tail: String = String::from_utf8_lossy(&output.stderr)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(format!("llama-mtmd-cli exited with {}: {tail}", output.status));
    }
    Ok(parse_asr_output(&String::from_utf8_lossy(&output.stdout)))
}

async fn run_stt(
    exe: &std::path::Path,
    model: &str,
    mmproj: &str,
    wav: &std::path::Path,
    prompt: &str,
    ngl: u32,
) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new(exe);
    crate::hidden::hide_tokio(&mut cmd);
    cmd.kill_on_drop(true)
        .arg("-m")
        .arg(model)
        .arg("--mmproj")
        .arg(mmproj)
        .arg("--audio")
        .arg(wav)
        .arg("-c")
        .arg("4096")
        .arg("-n")
        .arg("512")
        .arg("-ngl")
        .arg(ngl.to_string())
        .arg("--no-warmup")
        .arg("-p")
        .arg(prompt)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    tokio::time::timeout(std::time::Duration::from_secs(180), cmd.output())
        .await
        .map_err(|_| "llama-mtmd-cli timed out".to_string())?
        .map_err(|e| format!("Cannot run llama-mtmd-cli: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qwen3_asr_output() {
        let out = parse_asr_output("language English<asr_text>Hello there. How are you?");
        assert_eq!(out.language.as_deref(), Some("English"));
        assert_eq!(out.text, "Hello there. How are you?");
    }

    #[test]
    fn parses_without_marker() {
        let out = parse_asr_output("  just words  ");
        assert_eq!(out.language, None);
        assert_eq!(out.text, "just words");
    }
}
