//! Host hardware detection: RAM plus NVIDIA GPUs via nvidia-smi.
//! Best effort throughout — unknown hardware yields empty lists, never errors.
//! GPU and CUDA probes run once per process; hardware rarely changes
//! under a running app and each spawn costs hundreds of milliseconds.

use std::sync::OnceLock;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub vram_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub cpu_name: String,
    pub cpu_cores: u32,
    pub cpu_threads: u32,
    pub total_ram_mb: u64,
    pub available_ram_mb: u64,
    pub gpus: Vec<GpuInfo>,
    pub os: String,
    pub arch: String,
}

pub fn get_system_info() -> Result<SystemInfo> {
    let mut sys = sysinfo::System::new();
    sys.refresh_all();
    let cpu_name = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());
    Ok(SystemInfo {
        cpu_name,
        cpu_cores: sysinfo::System::physical_core_count().unwrap_or(0) as u32,
        cpu_threads: sys.cpus().len() as u32,
        total_ram_mb: sys.total_memory() / 1024 / 1024,
        available_ram_mb: sys.available_memory() / 1024 / 1024,
        gpus: cached_nvidia_gpus(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    })
}

static NVIDIA_GPUS: OnceLock<Vec<GpuInfo>> = OnceLock::new();
static CUDA_VERSION: OnceLock<Option<String>> = OnceLock::new();

fn cached_nvidia_gpus() -> Vec<GpuInfo> {
    NVIDIA_GPUS.get_or_init(detect_nvidia_gpus).clone()
}

/// `nvidia-smi --query-gpu=... --format=csv,noheader,nounits`, one GPU per line.
fn detect_nvidia_gpus() -> Vec<GpuInfo> {
    let out = crate::hidden::command("nvidia-smi")
        .args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_nvidia_smi(&String::from_utf8_lossy(&out.stdout))
}

/// Installed CUDA toolkit version. Prefers nvcc (toolkit) over
/// nvidia-smi (driver-supported version); either tool may be missing.
pub fn nvidia_cuda_version() -> Option<String> {
    CUDA_VERSION.get_or_init(|| cuda_from_nvcc().or_else(cuda_from_nvsmi)).clone()
}

fn cuda_from_nvcc() -> Option<String> {
    // `nvcc --version` reports `release X.Y,` on its own line.
    let out = crate::hidden::command("nvcc").arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_version_after(&String::from_utf8_lossy(&out.stdout), "release ")
}

fn cuda_from_nvsmi() -> Option<String> {
    let out = crate::hidden::command("nvidia-smi").output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_version_after(&String::from_utf8_lossy(&out.stdout), "CUDA Version: ")
}

fn parse_version_after(text: &str, marker: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains(marker))?;
    let rest = &line[line.find(marker)? + marker.len()..];
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(rest[..end].trim_end_matches('.').to_string())
}

fn parse_nvidia_smi(csv: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    for line in csv.lines() {
        let mut parts = line.splitn(2, ',');
        let name = parts.next().unwrap_or("").trim();
        let vram = parts
            .next()
            .unwrap_or("")
            .trim()
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        if name.is_empty() || vram == 0 {
            continue;
        }
        gpus.push(GpuInfo { name: name.to_string(), vram_mb: vram });
    }
    gpus
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi_csv() {
        let csv = "NVIDIA GeForce RTX 4090, 24564 MiB\nNVIDIA GeForce RTX 3090, 24576 MiB\n";
        let gpus = parse_nvidia_smi(csv);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(gpus[0].vram_mb, 24564);
        assert!(parse_nvidia_smi("").is_empty());
        assert!(parse_nvidia_smi("garbage line\n").is_empty());
    }

    #[test]
    fn version_markers_parse() {
        let smi = "| NVIDIA-SMI 550.54.15    Driver Version: 550.54.15    CUDA Version: 12.4     |";
        assert_eq!(parse_version_after(smi, "CUDA Version: ").as_deref(), Some("12.4"));
        let nvcc = "Cuda compilation tools, release 13.4, V13.4.99\nBuild cuda_13.4.r13.4/compiler.5539105_0";
        assert_eq!(parse_version_after(nvcc, "release ").as_deref(), Some("13.4"));
        assert_eq!(parse_version_after("no version here", "release "), None);
        assert_eq!(parse_version_after("release , nothing", "release "), None);
    }

    #[test]
    fn system_ram_is_sane() {
        let info = get_system_info().unwrap();
        assert!(info.total_ram_mb > 0);
        assert!(info.available_ram_mb <= info.total_ram_mb);
        assert!(!info.cpu_name.is_empty());
    }
}
