//! Quick Bench: one-rep `llama-bench` run with JSON history.
//! The binary resolves next to the active server binary; history caps at 200.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One bench at a time; parallel runs contend and starve past the timeout.
static BENCH_RUNNING: AtomicBool = AtomicBool::new(false);
/// Child pid of the live bench; app exit kills it (drop never runs then).
static BENCH_PID: Mutex<Option<u32>> = Mutex::new(None);

struct BenchGuard;

impl Drop for BenchGuard {
    fn drop(&mut self) {
        BENCH_RUNNING.store(false, Ordering::SeqCst);
        *BENCH_PID.lock().unwrap() = None;
    }
}

/// Kill the running bench, if any (app exit path).
pub fn kill_running() {
    let pid = *BENCH_PID.lock().unwrap();
    if let Some(pid) = pid {
        terminate(pid);
    }
}

fn terminate(pid: u32) {
    #[cfg(windows)]
    {
        let _ = crate::hidden::command("taskkill")
            .args(["/PID", &pid.to_string(), "/F", "/T"])
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = crate::hidden::command("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
}

/// Wire result: floats are t/s (nullable when unparsed), time is unix secs.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct BenchResult {
    pub model_path: String,
    pub model_name: String,
    pub n_prompt: u32,
    pub n_gen: u32,
    pub n_gpu_layers: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pp_tps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tg_tps: Option<f64>,
    pub status: String,
    pub timestamp: u32,
}

pub const HISTORY_CAP: usize = 200;

fn history_path() -> Result<PathBuf> {
    let config_path = crate::config::AppConfig::config_path()?;
    let base = config_path.parent().context("Config path has no parent")?;
    Ok(base.join("bench_history.json"))
}

fn load_from(path: &Path) -> Vec<BenchResult> {
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn save_to(path: &Path, results: &[BenchResult]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(results)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load_history() -> Vec<BenchResult> {
    history_path().map(|p| load_from(&p)).unwrap_or_default()
}

pub fn append_history(result: &BenchResult) -> Result<()> {
    let path = history_path()?;
    let mut results = load_from(&path);
    results.push(result.clone());
    if results.len() > HISTORY_CAP {
        results.drain(0..results.len() - HISTORY_CAP);
    }
    save_to(&path, &results)
}

pub fn clear_history() -> Result<()> {
    let path = history_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

fn bench_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "llama-bench.exe"
    } else {
        "llama-bench"
    }
}

fn find_in(dir: &Path, depth: u32) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some(bench_file_name()) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_in(&path, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Sibling of the active server binary (managed tree or custom dir).
pub fn bench_binary(
    config: &crate::config::AppConfig,
    runtimes_base: &Path,
) -> Result<PathBuf> {
    use crate::config::ActiveRuntime;
    let root = match &config.active_runtime {
        ActiveRuntime::Managed { build, backend_id } => {
            let rt = config
                .managed_runtimes
                .iter()
                .find(|r| r.build == *build && (backend_id.is_empty() || r.backend_id == *backend_id))
                .with_context(|| format!("Managed runtime b{build} not found"))?;
            runtimes_base.join(&rt.dir_name)
        }
        ActiveRuntime::Custom { index } => {
            let rt = config
                .custom_runtimes
                .get(*index as usize)
                .with_context(|| format!("Custom runtime index {index} not found"))?;
            rt.binary_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        }
        ActiveRuntime::None => anyhow::bail!("No active runtime"),
    };
    find_in(&root, 4)
        .with_context(|| format!("llama-bench not found under {}", root.display()))
}

/// Single rep of pp512 + tg128; appends to history and returns the entry.
pub async fn run_quick_bench(
    bench_bin: &Path,
    model_path: &str,
    n_gpu_layers: Option<i32>,
) -> Result<BenchResult> {
    if BENCH_RUNNING.swap(true, Ordering::SeqCst) {
        anyhow::bail!("A benchmark is already running");
    }
    let _guard = BenchGuard;

    let mut cmd = tokio::process::Command::new(bench_bin);
    crate::hidden::hide_tokio(&mut cmd);
    cmd.arg("-m")
        .arg(model_path)
        .arg("-p")
        .arg("512")
        .arg("-n")
        .arg("128")
        .arg("-r")
        .arg("1")
        .arg("--output")
        .arg("csv");
    if let Some(ngl) = n_gpu_layers {
        cmd.arg("-ngl").arg(ngl.to_string());
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    let child = cmd.spawn().context("Failed to spawn llama-bench")?;
    *BENCH_PID.lock().unwrap() = child.id();
    // On timeout the child drops here and kill_on_drop reaps it.
    let output = tokio::time::timeout(std::time::Duration::from_secs(300), child.wait_with_output())
        .await
        .context("Benchmark timed out after 300s")?
        .context("Failed to read llama-bench output")?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let (mut pp, mut tg) = parse_csv(&stdout).or_else(|| parse_csv(&stderr)).unwrap_or((None, None));
    if pp.is_none() || tg.is_none() {
        let (mpp, mtg) = parse_md(&stdout).or_else(|| parse_md(&stderr)).unwrap_or((None, None));
        if pp.is_none() {
            pp = mpp;
        }
        if tg.is_none() {
            tg = mtg;
        }
    }
    let status = if !output.status.success() {
        let tail = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
        let mut tail: String = tail.chars().take(200).collect();
        if tail.len() == 200 {
            tail.push('…');
        }
        format!("bench exited {0}: {tail}", output.status)
    } else if pp.is_none() && tg.is_none() {
        // Surface the first output line so forks with other formats are diagnosable.
        let first = stdout
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .chars()
            .take(120)
            .collect::<String>();
        if first.is_empty() {
            "ok (no throughput parsed: no output)".to_string()
        } else {
            format!("ok (no throughput parsed: {first})")
        }
    } else {
        "ok".to_string()
    };
    let result = BenchResult {
        model_path: model_path.to_string(),
        model_name: Path::new(model_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        n_prompt: 512,
        n_gen: 128,
        n_gpu_layers: n_gpu_layers.unwrap_or(-1),
        pp_tps: pp,
        tg_tps: tg,
        status,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0),
    };
    append_history(&result)?;
    Ok(result)
}

/// RFC 4180-ish split: commas inside quotes do not split, doubled quotes are literal.
fn split_csv(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' => quoted = true,
            ',' if !quoted => {
                fields.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    fields.push(cur.trim().to_string());
    fields
}

/// CSV rows carry n_prompt/n_gen/avg_ts; pp rows have n_gen=0, tg rows n_prompt=0.
fn parse_csv(out: &str) -> Option<(Option<f64>, Option<f64>)> {
    let mut lines = out.lines();
    let header: Vec<String> = split_csv(lines.next()?)
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let ip = header.iter().position(|h| h == "n_prompt")?;
    let ig = header.iter().position(|h| h == "n_gen")?;
    let it = header.iter().position(|h| h == "avg_ts")?;
    let (mut pp, mut tg) = (None, None);
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<String> = split_csv(line);
        if cols.len() <= ip.max(ig).max(it) {
            continue;
        }
        let (np, ng, ts): (u32, u32, f64) =
            match (cols[ip].parse().ok(), cols[ig].parse().ok(), cols[it].parse().ok()) {
                (Some(a), Some(b), Some(c)) => (a, b, c),
                _ => continue,
            };
        if np > 0 && ng == 0 {
            pp = Some(ts);
        } else if np == 0 && ng > 0 {
            tg = Some(ts);
        }
        if pp.is_some() && tg.is_some() {
            break;
        }
    }
    if pp.is_none() && tg.is_none() {
        return None;
    }
    Some((pp, tg))
}

/// Markdown fallback: `| … | pp512 | 540.99 |` and `| … | tg128 | 35.54 |`.
fn parse_md(out: &str) -> Option<(Option<f64>, Option<f64>)> {
    let (mut pp, mut tg) = (None, None);
    for line in out.lines() {
        if !line.contains('|') {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        for (i, cell) in cells.iter().enumerate() {
            let lower = cell.to_lowercase();
            let slot = if lower.starts_with("pp") {
                &mut pp
            } else if lower.starts_with("tg") {
                &mut tg
            } else {
                continue;
            };
            if slot.is_none() {
                *slot = cells
                    .get(i + 1)
                    .and_then(|n| n.split_whitespace().next())
                    .and_then(|n| n.parse::<f64>().ok());
            }
        }
        if pp.is_some() && tg.is_some() {
            break;
        }
    }
    if pp.is_none() && tg.is_none() {
        return None;
    }
    Some((pp, tg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_tells_pp_from_tg() {
        let out = "build_number,n_prompt,n_gen,avg_ts\n1,512,0,1200.5\n1,0,128,45.25\n";
        assert_eq!(parse_csv(out), Some((Some(1200.5), Some(45.25))));
    }

    #[test]
    fn csv_ignores_comments_and_blanks() {
        let out = "n_prompt,n_gen,avg_ts\n# note\n\n0,128,40.0\n";
        assert_eq!(parse_csv(out), Some((None, Some(40.0))));
    }

    #[test]
    fn csv_handles_quoted_commas() {
        let out = "cpu_info,n_prompt,n_gen,avg_ts\n\"Ryzen 5, with graphics\",512,0,1200.5\n\"Ryzen 5, with graphics\",0,128,45.25\n";
        assert_eq!(parse_csv(out), Some((Some(1200.5), Some(45.25))));
    }

    #[test]
    fn csv_without_throughput_is_none() {
        assert_eq!(parse_csv("n_prompt,n_gen,avg_ts\n"), None);
        assert_eq!(parse_csv("no header here"), None);
    }

    #[test]
    fn md_reads_pp_tg_cells() {
        let out = "| model | test | t/s |\n| m | pp512 | 540.99 |\n| m | tg128 | 35.54 |\n";
        assert_eq!(parse_md(out), Some((Some(540.99), Some(35.54))));
    }

    #[test]
    fn md_without_readings_is_none() {
        assert_eq!(parse_md("| a | b |\n"), None);
    }

    #[test]
    fn history_round_trips_and_caps() {
        let dir = std::env::temp_dir().join(format!("werk-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bench_history.json");
        let entry = BenchResult {
            model_path: "/m/a.gguf".into(),
            model_name: "a.gguf".into(),
            n_prompt: 512,
            n_gen: 128,
            n_gpu_layers: -1,
            pp_tps: Some(1.0),
            tg_tps: Some(2.0),
            status: "ok".into(),
            timestamp: 1,
        };
        let mut results = vec![entry.clone()];
        for i in 0..(HISTORY_CAP + 5) {
            let mut e = entry.clone();
            e.timestamp = i as u32;
            results.push(e);
        }
        let mut capped = results.clone();
        if capped.len() > HISTORY_CAP {
            capped.drain(0..capped.len() - HISTORY_CAP);
        }
        save_to(&path, &capped).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.len(), HISTORY_CAP);
        assert_eq!(loaded.last().unwrap().timestamp, (HISTORY_CAP + 5 - 1) as u32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finder_spots_the_binary() {
        let dir = std::env::temp_dir().join(format!("werk-bench-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nested = dir.join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join(bench_file_name()), "x").unwrap();
        assert!(find_in(&dir, 4).is_some());
        assert!(find_in(&dir, 1).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
