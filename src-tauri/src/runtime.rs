//! Runtime management: fetch llama.cpp releases, score assets for this
//! host, download + install managed builds, or point at custom binaries.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::{ActiveRuntime, AppConfig, CustomRuntime, ManagedRuntime};
use crate::hardware::GpuInfo;

#[derive(Debug, Clone)]
pub struct BackendInfo {
    pub id: String,
    pub label: String,
    pub available: bool,
}

/// CPU always; CUDA with an NVIDIA GPU; Metal on Apple Silicon.
/// (Vulkan/ROCm/SYCL detection is M-polish; those users get CPU builds.)
pub fn detect_backends(gpus: &[GpuInfo]) -> Vec<BackendInfo> {
    let has_nvidia = gpus.iter().any(|g| {
        let n = g.name.to_lowercase();
        n.contains("nvidia") || n.contains("geforce") || n.contains("rtx") || n.contains("gtx")
    });
    vec![
        BackendInfo { id: "cuda".into(), label: "CUDA".into(), available: has_nvidia },
        BackendInfo {
            id: "metal".into(),
            label: "Metal".into(),
            available: cfg!(all(target_os = "macos", target_arch = "aarch64")),
        },
        BackendInfo { id: "cpu".into(), label: "CPU".into(), available: true },
    ]
}

/// First available non-CPU backend, else CPU.
pub fn preferred_backend_id(backends: &[BackendInfo]) -> String {
    backends
        .iter()
        .find(|b| b.available && b.id != "cpu")
        .map(|b| b.id.clone())
        .unwrap_or_else(|| "cpu".to_string())
}

/// Strict `b<build>` tag; None for semver and other tags.
pub fn parse_build_tag(tag: &str) -> Option<u32> {
    let digits = tag.strip_prefix('b')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

/// Newest `b<build>` nightly; the "latest" pointer aims at semver
/// stable, which ships no binaries.
pub async fn fetch_latest_release(client: &reqwest::Client) -> Result<ReleaseInfo> {
    let releases: Vec<ReleaseInfo> = client
        .get("https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=100")
        .header("User-Agent", "werk/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("Fetching llama.cpp releases")?
        .error_for_status()?
        .json()
        .await?;
    newest_nightly(releases).context("No nightly build release found")
}

/// First release with a `b<build>` tag (the list arrives newest-first).
fn newest_nightly(releases: Vec<ReleaseInfo>) -> Option<ReleaseInfo> {
    releases.into_iter().find(|r| parse_build_tag(&r.tag_name).is_some())
}

/// An asset that can install here: right archive, platform, and arch.
/// Checksums, signatures, sources, and foreign OS/arch builds never qualify.
fn usable_asset(name: &str) -> bool {
    let lower = name.to_lowercase();
    if !(lower.ends_with(".zip") || lower.ends_with(".tar.gz") || lower.ends_with(".tgz")) {
        return false;
    }
    if lower.contains("sha256") || lower.contains("source") || lower.ends_with(".sig") {
        return false;
    }
    let platform_ok = match std::env::consts::OS {
        "linux" => lower.contains("linux") || lower.contains("ubuntu"),
        "windows" => lower.contains("win"),
        "macos" => lower.contains("macos") || lower.contains("darwin"),
        _ => false,
    };
    if !platform_ok {
        return false;
    }
    match std::env::consts::ARCH {
        "x86_64" => lower.contains("x64") || lower.contains("amd64") || lower.contains("x86_64"),
        "aarch64" => lower.contains("arm64") || lower.contains("aarch64"),
        _ => true,
    }
}

/// Version suffix from a name (`cuda-13.4` -> ` 13.4`); empty when absent.
/// The `cudart-` bundle prefix is not a version marker.
fn cuda_suffix(lower: &str) -> String {
    let mut rest = "";
    let mut search = lower;
    while let Some(i) = search.find("cuda") {
        let after = &search[i + 4..];
        let next = after.chars().next();
        if next.is_some_and(|c| c == '-' || c == '_' || c.is_ascii_digit()) {
            rest = after;
        }
        search = after;
    }
    let ver: String = rest
        .trim_start_matches(['-', '_'])
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let ver = ver.trim_matches(['.', '-']).replace('-', ".");
    if ver.is_empty() {
        String::new()
    } else {
        format!(" {ver}")
    }
}

/// Backend guess from the asset name: (id, label, base score).
/// The score doubles as the Recommended signal (>= 90).
fn detect_asset_backend(lower: &str) -> (String, String, i64) {
    if lower.contains("cuda") {
        ("cuda".into(), format!("CUDA{}", cuda_suffix(lower)), 100)
    } else if lower.contains("rocm") || lower.contains("hip") {
        ("rocm".into(), "ROCm".into(), 90)
    } else if lower.contains("metal") {
        ("metal".into(), "Metal".into(), 95)
    } else if lower.contains("vulkan") {
        ("vulkan".into(), "Vulkan".into(), 70)
    } else if lower.contains("sycl") {
        ("sycl".into(), "SYCL".into(), 60)
    } else if lower.contains("openvino") {
        ("openvino".into(), "OpenVINO".into(), 50)
    } else if lower.contains("opencl") {
        ("opencl".into(), "OpenCL".into(), 40)
    } else if lower.contains("avx512") {
        ("cpu-avx512".into(), "CPU (AVX-512)".into(), 30)
    } else if lower.contains("avx2") {
        ("cpu-avx2".into(), "CPU (AVX2)".into(), 25)
    } else if lower.contains("noavx") || lower.contains("no-avx") {
        ("cpu-noavx".into(), "CPU (no AVX)".into(), 10)
    } else if lower.contains("avx") {
        ("cpu-avx".into(), "CPU (AVX)".into(), 20)
    } else if lower.contains("cpu") {
        ("cpu".into(), "CPU".into(), 20)
    } else if cfg!(target_os = "macos") {
        ("metal".into(), "Metal".into(), 95)
    } else {
        ("cpu".into(), "CPU".into(), 20)
    }
}

/// Every usable asset with its detected backend, highest score first.
/// Unavailable backends lose 200; a CUDA build for another toolkit drops
/// below the Recommended threshold.
pub struct ScoredAsset<'a> {
    pub asset: &'a ReleaseAsset,
    pub backend_id: String,
    pub backend_label: String,
    pub score: i64,
}

pub fn score_release<'a>(
    release: &'a ReleaseInfo,
    backends: &[BackendInfo],
    cuda_version: Option<&str>,
) -> Vec<ScoredAsset<'a>> {
    let available: Vec<&str> = backends
        .iter()
        .filter(|b| b.available)
        .map(|b| b.id.as_str())
        .collect();
    let mut out = Vec::new();
    for asset in &release.assets {
        if !usable_asset(&asset.name) {
            continue;
        }
        let lower = asset.name.to_lowercase();
        let (backend_id, backend_label, mut score) = detect_asset_backend(&lower);
        let usable =
            backend_id.starts_with("cpu") || available.iter().any(|b| backend_id.starts_with(b));
        if !usable {
            score -= 200;
        }
        // CUDA builds for another toolkit never list when ours is known.
        if backend_id == "cuda" {
            if let Some(sys) = cuda_version {
                if cuda_suffix(&lower).trim() != sys.trim() {
                    continue;
                }
            }
        }
        out.push(ScoredAsset { asset, backend_id, backend_label, score });
    }
    out.sort_by(|a, b| b.score.cmp(&a.score));
    out
}

/// Best usable asset for this host; None when nothing scores above zero.
pub fn pick_asset<'a>(
    release: &'a ReleaseInfo,
    backends: &[BackendInfo],
    cuda_version: Option<&str>,
) -> Option<ScoredAsset<'a>> {
    score_release(release, backends, cuda_version)
        .into_iter()
        .filter(|s| s.score >= 0)
        .max_by_key(|s| s.score)
}

/// A `llama-server` binary found under a directory.
pub struct FoundBinary {
    pub path: PathBuf,
    /// Parent dir relative to the scan root (`build/bin`, or the dir name).
    pub label: String,
}

/// All server binaries under `dir` (recursive, depth 5), sorted by path.
pub fn scan_binaries(dir: &Path) -> Vec<FoundBinary> {
    let target = if cfg!(target_os = "windows") { "llama-server.exe" } else { "llama-server" };
    let mut hits = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((current, depth)) = stack.pop() {
        if depth > 5 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            } else if path.file_name().and_then(|n| n.to_str()) == Some(target) {
                let label = path
                    .parent()
                    .and_then(|p| p.strip_prefix(dir).ok())
                    .map(|p| p.to_string_lossy().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        path.parent()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().to_string())
                    })
                    .unwrap_or_else(|| target.to_string());
                hits.push(FoundBinary { path, label });
            }
        }
    }
    hits.sort_by(|a, b| a.path.cmp(&b.path));
    hits
}

fn sanitize_dir_name(tag: &str, backend: &str) -> String {
    let base = format!("{tag}-{backend}");
    base.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

/// Download + unzip an asset into `runtimes_base/<dir>`, returning the dir.
pub async fn install_asset(
    client: &reqwest::Client,
    asset: &ReleaseAsset,
    backend_id: &str,
    tag_name: &str,
    runtimes_base: &Path,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    let dir = runtimes_base.join(sanitize_dir_name(tag_name, backend_id));
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("{}.part", asset.name));
    // A previous attempt may have finished the bytes but failed to extract.
    let complete = asset.size.is_some_and(|total| {
        total > 0 && std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) == total
    });
    if complete {
        on_progress(asset.size.unwrap_or(0), asset.size);
    } else {
        crate::download::download_to(
            client,
            &asset.browser_download_url,
            &tmp,
            cancel,
            |p| on_progress(p.downloaded, p.total),
        )
        .await?;
    }
    extract_archive(&tmp, &dir, &asset.name)
        .with_context(|| format!("Extracting {}", asset.name))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(dir)
}

/// Dispatch on the asset name: the download lands in a `.part` file,
// which would misroute zip archives into the tar reader.
fn extract_archive(archive: &Path, dir: &Path, asset_name: &str) -> Result<()> {
    if asset_name.to_lowercase().ends_with(".zip") {
        extract_zip(archive, dir)
    } else {
        extract_tar_gz(archive, dir)
    }
}

fn extract_zip(archive: &Path, dir: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        let dest = dir.join(path);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = entry.unix_mode() {
                    let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode));
                }
            }
        }
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, dir: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    tar.unpack(dir)?;
    Ok(())
}

/// Find `llama-server(.exe)` under a runtime dir (recursive, shallow first).
pub fn find_server_binary(dir: &Path) -> Option<PathBuf> {
    let target = if cfg!(target_os = "windows") { "llama-server.exe" } else { "llama-server" };
    let mut stack = vec![dir.to_path_buf()];
    let mut depth = 0;
    while let Some(current) = stack.pop() {
        depth += 1;
        if depth > 8 {
            continue;
        }
        let entries = std::fs::read_dir(&current).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(target) {
                return Some(path);
            }
        }
    }
    None
}

pub fn runtimes_base_dir() -> Result<PathBuf> {
    let data = crate::config::AppConfig::config_path()?;
    Ok(data.parent().unwrap_or(Path::new(".")).join("runtimes").to_path_buf())
}

pub fn register_downloaded_runtime(
    config: &mut AppConfig,
    build: u32,
    tag_name: &str,
    backend_id: &str,
    backend_label: &str,
    asset_name: &str,
    dir_name: String,
) {
    config.managed_runtimes.retain(|r| {
        !(r.build == build && r.backend_id == backend_id)
    });
    config.managed_runtimes.push(ManagedRuntime {
        build,
        tag_name: tag_name.to_string(),
        backend_id: backend_id.to_string(),
        backend_label: backend_label.to_string(),
        asset_name: asset_name.to_string(),
        dir_name: dir_name.clone(),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0),
    });
    // Same-backend only: other accelerators stay installed on purpose.
    if config.auto_delete_old_runtimes {
        if let Ok(base) = runtimes_base_dir() {
            let stale: Vec<(u32, String, String)> = config
                .managed_runtimes
                .iter()
                .filter(|r| r.backend_id == backend_id && r.build != build)
                .map(|r| (r.build, r.backend_id.clone(), r.dir_name.clone()))
                .collect();
            for (old_build, old_backend, old_dir) in stale {
                let _ = std::fs::remove_dir_all(base.join(old_dir));
                config
                    .managed_runtimes
                    .retain(|r| !(r.build == old_build && r.backend_id == old_backend));
            }
        }
    }
    config.managed_runtimes.sort_by(|a, b| b.build.cmp(&a.build));
    config.active_runtime = ActiveRuntime::Managed {
        build,
        backend_id: backend_id.to_string(),
    };
}

pub fn add_custom_runtime(config: &mut AppConfig, binary_path: PathBuf) -> u32 {
    if let Some(i) = config.custom_runtimes.iter().position(|c| c.binary_path == binary_path) {
        let index = i as u32;
        config.active_runtime = ActiveRuntime::Custom { index };
        return index;
    }
    let label = binary_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Custom".to_string());
    let index = config.custom_runtimes.len() as u32;
    config.custom_runtimes.push(CustomRuntime { label, binary_path });
    config.active_runtime = ActiveRuntime::Custom { index };
    index
}

pub fn delete_managed_runtime(
    config: &mut AppConfig,
    build: u32,
    backend_id: &str,
    runtimes_base: &Path,
) -> Result<()> {
    let pos = config
        .managed_runtimes
        .iter()
        .position(|r| r.build == build && r.backend_id == backend_id);
    let Some(pos) = pos else {
        anyhow::bail!("Runtime b{build} ({backend_id}) is not installed");
    };
    let rt = config.managed_runtimes.remove(pos);
    let _ = std::fs::remove_dir_all(runtimes_base.join(&rt.dir_name));
    if config.active_runtime
        == (ActiveRuntime::Managed { build, backend_id: backend_id.to_string() })
    {
        config.active_runtime = ActiveRuntime::None;
    }
    Ok(())
}

pub fn remove_custom_runtime(config: &mut AppConfig, index: u32) -> Result<()> {
    if (index as usize) >= config.custom_runtimes.len() {
        anyhow::bail!("Custom runtime index {index} not found");
    }
    if config.active_runtime == (ActiveRuntime::Custom { index }) {
        anyhow::bail!("Cannot remove the active runtime");
    }
    config.custom_runtimes.remove(index as usize);
    if let ActiveRuntime::Custom { index: ref mut active } = config.active_runtime {
        if *active > index {
            *active -= 1;
        }
    }
    Ok(())
}

/// Resolve the server binary for the active runtime.
pub fn server_binary(config: &AppConfig, runtimes_base: &Path) -> Result<PathBuf> {
    match &config.active_runtime {
        ActiveRuntime::Managed { build, backend_id } => {
            let rt = config
                .managed_runtimes
                .iter()
                .find(|r| r.build == *build && (backend_id.is_empty() || r.backend_id == *backend_id))
                .with_context(|| format!("Managed runtime b{build} not found"))?;
            let dir = runtimes_base.join(&rt.dir_name);
            find_server_binary(&dir)
                .with_context(|| format!("llama-server not found in {}", dir.display()))
        }
        ActiveRuntime::Custom { index } => {
            let rt = config
                .custom_runtimes
                .get(*index as usize)
                .with_context(|| format!("Custom runtime index {index} not found"))?;
            if !rt.binary_path.is_file() {
                anyhow::bail!("Custom binary missing: {}", rt.binary_path.display());
            }
            Ok(rt.binary_path.clone())
        }
        ActiveRuntime::None => anyhow::bail!("No active runtime"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_with(names: &[&str]) -> ReleaseInfo {
        ReleaseInfo {
            tag_name: "b9999".into(),
            assets: names
                .iter()
                .map(|n| ReleaseAsset {
                    name: n.to_string(),
                    browser_download_url: format!("https://example.com/{n}"),
                    size: None,
                })
                .collect(),
        }
    }

    fn cuda_backends() -> Vec<BackendInfo> {
        vec![
            BackendInfo { id: "cuda".into(), label: "CUDA".into(), available: true },
            BackendInfo { id: "cpu".into(), label: "CPU".into(), available: true },
        ]
    }

    /// Asset name carrying this host's OS/arch tokens.
    fn host_asset(rest: &str) -> String {
        let os = match std::env::consts::OS {
            "windows" => "win",
            "macos" => "macos",
            _ => "linux",
        };
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            _ => "x64",
        };
        format!("llama-b9999-bin-{os}-{arch}-{rest}.zip")
    }

    #[test]
    fn scoring_picks_cuda_build_for_cuda_hosts() {
        let cuda = host_asset("cuda-12-4");
        let cpu = host_asset("cpu");
        let release = release_with(&[cuda.as_str(), cpu.as_str(), "llama-b9999-src.zip"]);
        let picked = pick_asset(&release, &cuda_backends(), Some("12.4")).unwrap();
        assert_eq!(picked.backend_id, "cuda");
        assert!(picked.asset.name.contains("cuda-12-4"), "{}", picked.asset.name);
    }

    #[test]
    fn scoring_rejects_wrong_platform_and_sources() {
        assert!(!usable_asset("llama-src.zip"));
        assert!(!usable_asset("llama-b1-bin-win-cpu-x64.zip.sha256"));
        if cfg!(target_os = "windows") {
            assert!(!usable_asset("llama-b1-bin-linux-cpu-x64.zip"));
            assert!(usable_asset("llama-b1-bin-win-cpu-x64.zip"));
        } else if cfg!(target_os = "macos") {
            assert!(usable_asset("llama-b1-bin-macos-arm64.tar.gz"));
        } else {
            assert!(usable_asset("llama-b1-bin-linux-cpu-x64.tar.gz"));
        }
    }

    #[test]
    fn cpu_fallback_without_gpu() {
        let name = host_asset("cpu");
        let release = release_with(&[name.as_str()]);
        let backends =
            vec![BackendInfo { id: "cpu".into(), label: "CPU".into(), available: true }];
        let picked = pick_asset(&release, &backends, None).unwrap();
        assert_eq!(picked.backend_id, "cpu");
        assert!(picked.asset.name.contains("cpu"));
    }

    #[test]
    fn register_activate_and_delete_round_trip() {
        let base = std::env::temp_dir().join(format!("werk-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("b9999-cuda")).unwrap();
        let exe = if cfg!(target_os = "windows") { "llama-server.exe" } else { "llama-server" };
        std::fs::write(base.join("b9999-cuda").join(exe), b"x").unwrap();
        let mut config = AppConfig::default();
        register_downloaded_runtime(&mut config, 9999, "b9999", "cuda", "CUDA", "a.zip", "b9999-cuda".into());
        assert!(matches!(
            config.active_runtime,
            ActiveRuntime::Managed { build: 9999, .. }
        ));
        // Re-registering the same build replaces instead of duplicating.
        register_downloaded_runtime(&mut config, 9999, "b9999", "cuda", "CUDA", "a2.zip", "b9999-cuda".into());
        assert_eq!(config.managed_runtimes.len(), 1);
        let bin = server_binary(&config, &base).unwrap();
        assert!(bin.is_file());
        delete_managed_runtime(&mut config, 9999, "cuda", &base).unwrap();
        assert!(config.managed_runtimes.is_empty());
        assert_eq!(config.active_runtime, ActiveRuntime::None);
        assert!(!base.join("b9999-cuda").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    fn managed(build: u32, backend_id: &str) -> ManagedRuntime {
        ManagedRuntime {
            build,
            tag_name: format!("b{build}"),
            backend_id: backend_id.into(),
            backend_label: backend_id.to_uppercase(),
            asset_name: format!("b{build}-{backend_id}.zip"),
            dir_name: format!("b{build}-{backend_id}"),
            installed_at: 0,
        }
    }

    #[test]
    fn register_keeps_old_builds_when_auto_delete_off() {
        let mut config = AppConfig::default();
        config.managed_runtimes.push(managed(3000, "cuda"));
        register_downloaded_runtime(&mut config, 5000, "b5000", "cuda", "CUDA", "a.zip", "b5000-cuda".into());
        assert!(config.managed_runtimes.iter().any(|r| r.build == 3000));
        assert!(config.managed_runtimes.iter().any(|r| r.build == 5000));
    }

    #[test]
    fn register_auto_deletes_only_same_backend() {
        let mut config = AppConfig::default();
        config.auto_delete_old_runtimes = true;
        config.managed_runtimes.push(managed(3000, "cuda"));
        config.managed_runtimes.push(managed(3000, "vulkan"));
        register_downloaded_runtime(&mut config, 5000, "b5000", "cuda", "CUDA", "a.zip", "b5000-cuda".into());
        assert!(!config.managed_runtimes.iter().any(|r| r.build == 3000 && r.backend_id == "cuda"));
        assert!(config.managed_runtimes.iter().any(|r| r.build == 3000 && r.backend_id == "vulkan"));
        assert!(config.managed_runtimes.iter().any(|r| r.build == 5000 && r.backend_id == "cuda"));
    }

    #[test]
    fn custom_runtime_add_remove() {
        let mut config = AppConfig::default();
        let idx = add_custom_runtime(&mut config, PathBuf::from("/opt/llama/llama-server"));
        assert_eq!(idx, 0);
        assert_eq!(config.active_runtime, ActiveRuntime::Custom { index: 0 });
        assert!(remove_custom_runtime(&mut config, 0).is_err(), "active removal refused");
        let idx2 = add_custom_runtime(&mut config, PathBuf::from("/other/srv"));
        assert_eq!(idx2, 1);
        remove_custom_runtime(&mut config, 0).unwrap();
        assert_eq!(config.active_runtime, ActiveRuntime::Custom { index: 0 });
    }

    #[test]
    fn build_tags_strict() {
        assert_eq!(parse_build_tag("b11177"), Some(11177));
        assert_eq!(parse_build_tag("v1.2.3"), None);
        assert_eq!(parse_build_tag("nightly"), None);
        assert_eq!(parse_build_tag("b"), None);
        assert_eq!(parse_build_tag("b12x"), None);
    }

    #[test]
    fn newest_nightly_skips_semver() {
        let mk = |tag: &str| ReleaseInfo { tag_name: tag.into(), assets: vec![] };
        let releases = vec![mk("v1.0.0"), mk("b11177"), mk("b11170")];
        assert_eq!(newest_nightly(releases).map(|r| r.tag_name), Some("b11177".into()));
        assert!(newest_nightly(vec![mk("v1.0.0")]).is_none());
    }

    #[test]
    fn cuda_suffix_reads_dashes_and_dots() {
        assert_eq!(cuda_suffix("cudart-llama-bin-win-cuda-13.4-x64.zip"), " 13.4");
        assert_eq!(cuda_suffix("llama-b11177-bin-win-cuda-12.4-x64.zip"), " 12.4");
        assert_eq!(cuda_suffix("llama-b11177-bin-win-cpu-x64.zip"), "");
    }

    fn scored_names(release: &ReleaseInfo, backends: &[BackendInfo], cuda: Option<&str>) -> Vec<(String, i64)> {
        score_release(release, backends, cuda)
            .into_iter()
            .map(|s| (s.asset.name.clone(), s.score))
            .collect()
    }

    fn win_cuda_host() -> Vec<BackendInfo> {
        vec![
            BackendInfo { id: "cuda".into(), label: "CUDA".into(), available: true },
            BackendInfo { id: "cpu".into(), label: "CPU".into(), available: true },
        ]
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_cuda_host_prefers_matching_cuda() {
        let mk = |name: &str| ReleaseAsset {
            name: name.into(),
            browser_download_url: "http://x".into(),
            size: Some(1),
        };
        let release = ReleaseInfo {
            tag_name: "b11177".into(),
            assets: vec![
                mk("cudart-llama-bin-win-cuda-12.4-x64.zip"),
                mk("cudart-llama-bin-win-cuda-13.4-x64.zip"),
                mk("llama-b11177-bin-win-cpu-x64.zip"),
                mk("llama-b11177-bin-win-cpu-arm64.zip"),
                mk("llama-b11177-bin-ubuntu-cuda-13.4-x64.tar.gz"),
                mk("llama-b11177-ui.tar.gz"),
                mk("nightly-tag.txt"),
            ],
        };
        let scored = scored_names(&release, &win_cuda_host(), Some("13.4"));
        assert_eq!(scored[0].0, "cudart-llama-bin-win-cuda-13.4-x64.zip");
        assert!(scored[0].1 >= 90, "matching CUDA stays recommended");
        assert!(
            !scored.iter().any(|(n, _)| n.contains("cuda-12.4")),
            "other toolkit builds never list"
        );
        assert!(scored.iter().any(|(n, _)| n.contains("win-cpu-x64")));
        assert!(!scored.iter().any(|n| n.0.contains("arm64")), "wrong arch out");
        assert!(!scored.iter().any(|n| n.0.contains("ubuntu")), "foreign OS out");
        assert!(!scored.iter().any(|n| n.0.contains("ui.tar")), "UI bundle out");
        let picked = pick_asset(&release, &win_cuda_host(), Some("13.4")).unwrap();
        assert_eq!(picked.asset.name, "cudart-llama-bin-win-cuda-13.4-x64.zip");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn cpu_only_host_falls_back_to_cpu() {
        let mk = |name: &str| ReleaseAsset {
            name: name.into(),
            browser_download_url: "http://x".into(),
            size: Some(1),
        };
        let release = ReleaseInfo {
            tag_name: "b11177".into(),
            assets: vec![
                mk("cudart-llama-bin-win-cuda-13.4-x64.zip"),
                mk("llama-b11177-bin-win-cpu-x64.zip"),
            ],
        };
        let cpu = vec![BackendInfo { id: "cpu".into(), label: "CPU".into(), available: true }];
        let picked = pick_asset(&release, &cpu, None).unwrap();
        assert_eq!(picked.asset.name, "llama-b11177-bin-win-cpu-x64.zip");
    }

    #[test]
    fn backend_table_ranks_accelerators() {
        let rank = |name: &str| detect_asset_backend(&name.to_lowercase()).2;
        assert!(rank("x-cuda-13.4.zip") > rank("x-rocm-10.0.zip"));
        assert!(rank("x-rocm-10.0.zip") > rank("x-vulkan-x64.zip"));
        assert!(rank("x-vulkan-x64.zip") > rank("x-sycl-x64.zip"));
        assert!(rank("x-cpu-x64.zip") > rank("x-cpu-noavx.zip"));
    }

    #[test]
    fn archives_dispatch_by_asset_name() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("werk-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Zip bytes in a `.part` file, as real downloads land.
        let zip_part = dir.join("a.zip.part");
        {
            let f = std::fs::File::create(&zip_part).unwrap();
            let mut w = zip::ZipWriter::new(f);
            w.start_file("hello.txt", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"hi").unwrap();
            w.finish().unwrap();
        }
        let out_zip = dir.join("out-zip");
        extract_archive(&zip_part, &out_zip, "a.zip").unwrap();
        assert_eq!(std::fs::read_to_string(out_zip.join("hello.txt")).unwrap(), "hi");
        // Tarball in a `.part` file.
        let tar_part = dir.join("a.tar.gz.part");
        {
            let f = std::fs::File::create(&tar_part).unwrap();
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            header.set_size(2);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, "hello.txt", b"hi".as_slice()).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let out_tar = dir.join("out-tar");
        extract_archive(&tar_part, &out_tar, "a.tar.gz").unwrap();
        assert_eq!(std::fs::read_to_string(out_tar.join("hello.txt")).unwrap(), "hi");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
