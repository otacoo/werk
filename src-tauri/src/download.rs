//! Resumable file downloads with progress and a cancel hook.
//! Partials stay at the destination path so a retry resumes them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::Digest;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Parse a `Content-Range: bytes 100-200/1234` total.
fn content_range_total(header: &str) -> Option<u64> {
    header.rsplit('/').next()?.trim().parse().ok()
}

/// GET `url` into `dest`, resuming a partial file. `cancel` is polled
/// between chunks; progress reports cumulative bytes. With `expected_sha256`
/// the bytes are hashed as they stream (no second pass over the file) and a
/// mismatch removes the download.
pub async fn download_to(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    mut on_progress: impl FnMut(DownloadProgress),
    expected_sha256: Option<&str>,
) -> Result<PathBuf> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match download_from(client, url, dest, cancel, &mut on_progress, expected_sha256).await {
        // Our offset is past EOF (stale oversized partial): start over.
        Err(e) if e.downcast_ref::<RangeUnsatisfiable>().is_some() => {
            let _ = std::fs::remove_file(dest);
            download_from(client, url, dest, cancel, &mut on_progress, expected_sha256).await
        }
        other => other,
    }
}

/// The server refused our resume offset: nothing beyond it exists.
#[derive(Debug)]
struct RangeUnsatisfiable;

impl std::fmt::Display for RangeUnsatisfiable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server refused the resume offset")
    }
}

impl std::error::Error for RangeUnsatisfiable {}

async fn download_from(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    on_progress: &mut impl FnMut(DownloadProgress),
    expected_sha256: Option<&str>,
) -> Result<PathBuf> {
    let have = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let mut request = client.get(url);
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let resp = request.send().await.with_context(|| format!("GET {url}"))?;
    if resp.status().as_u16() == 416 {
        // EOF offset means the file is already complete; a larger one is a
        // stale oversized partial that must start over.
        let remote = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(content_range_total);
        if remote == Some(have) {
            if let Some(expected) = expected_sha256 {
                let mut hasher = sha2::Sha256::new();
                hash_file(&mut hasher, dest).await?;
                check_sha256(&hex(&hasher.finalize()), expected, dest)?;
            }
            return Ok(dest.to_path_buf());
        }
        return Err(RangeUnsatisfiable.into());
    }
    if !resp.status().is_success() && resp.status().as_u16() != 206 {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Download failed ({status}): {}", text.chars().take(300).collect::<String>());
    }
    // 206 appends; anything else restarts from zero.
    let resumed = resp.status().as_u16() == 206;
    let total = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(content_range_total)
        .or_else(|| {
            resp.headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(|len| if resumed { len + have } else { len })
        });
    // Refuse to start when the remaining bytes do not fit the disk.
    if let Some(total) = total {
        let remaining = total.saturating_sub(if resumed { have } else { 0 });
        if let Some(free) = crate::hardware::free_space_for(dest) {
            const RESERVE: u64 = 256 * 1024 * 1024;
            if remaining.saturating_add(RESERVE) > free {
                anyhow::bail!(
                    "Not enough disk space: {:.1} GB needed, {:.1} GB free",
                    remaining as f64 / 1e9,
                    free as f64 / 1e9
                );
            }
        }
    }
    // A resumed file is still verified from byte zero: hash the prefix that
    // is already on disk, then the incoming stream.
    let mut hasher = expected_sha256.map(|_| sha2::Sha256::new());
    if let (Some(hasher), true) = (hasher.as_mut(), resumed) {
        hash_file(hasher, dest).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!resumed)
        .open(dest)
        .await?;
    if resumed {
        file.seek(std::io::SeekFrom::Start(have)).await?;
    }
    let mut downloaded = if resumed { have } else { 0 };
    on_progress(DownloadProgress { downloaded, total });
    use futures::StreamExt;
    // Chunks arrive far faster than the UI can use them; report on a clock.
    let mut last_emit = std::time::Instant::now();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel() {
            anyhow::bail!("Download cancelled");
        }
        let chunk = chunk.context("Download stream failed")?;
        file.write_all(&chunk).await?;
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&chunk);
        }
        downloaded += chunk.len() as u64;
        if last_emit.elapsed() >= std::time::Duration::from_millis(100) {
            last_emit = std::time::Instant::now();
            on_progress(DownloadProgress { downloaded, total });
        }
    }
    on_progress(DownloadProgress { downloaded, total });
    file.flush().await?;
    if let Some(total) = total {
        if downloaded < total {
            anyhow::bail!("Download ended early: {downloaded} of {total} bytes");
        }
    }
    if let (Some(hasher), Some(expected)) = (hasher, expected_sha256) {
        check_sha256(&hex(&hasher.finalize()), expected, dest)?;
    }
    Ok(dest.to_path_buf())
}

/// Stream a file through the hasher (a resume prefix, or a completed part).
async fn hash_file(hasher: &mut sha2::Sha256, path: &Path) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
    }
}

/// Compare a finished download against its pin; a mismatch removes the file
/// so a retry starts clean.
fn check_sha256(actual: &str, expected: &str, dest: &Path) -> Result<()> {
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    let _ = std::fs::remove_file(dest);
    anyhow::bail!(
        "Checksum mismatch: expected {expected}, got {actual}. The file changed upstream; werk may need an update."
    )
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn content_range_total_parses() {
        assert_eq!(content_range_total("bytes 100-200/1234"), Some(1234));
        assert_eq!(content_range_total("bytes */999"), Some(999));
        assert_eq!(content_range_total("garbage"), None);
    }

    /// Slow local server; the stop flag must abort the stream mid-flight.
    #[tokio::test]
    async fn cancel_aborts_mid_stream() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut sock, _) = listener.accept().await.unwrap();
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000000\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let chunk = vec![0xABu8; 16384];
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                if sock.write_all(&chunk).await.is_err() {
                    break;
                }
            }
        });
        let client = reqwest::Client::new();
        let dir = std::env::temp_dir().join(format!("werk-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("f.bin");
        let cancel = Arc::new(AtomicBool::new(false));
        let stop_flag = cancel.clone();
        let mut flowed = 0u64;
        let mut stopped = false;
        // Cancel only after bytes actually flowed, from the progress callback:
        // a wall-clock timer would race the server's first chunk on slow CI.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            download_to(
                &client,
                &format!("http://{addr}/f.bin"),
                &dest,
                &move || cancel.load(Ordering::SeqCst),
                |p| {
                    flowed = p.downloaded;
                    if p.downloaded > 0 && !stopped {
                        stopped = true;
                        stop_flag.store(true, Ordering::SeqCst);
                    }
                },
                None,
            ),
        )
        .await
        .expect("download timed out");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
        assert!(flowed > 0, "bytes flowed before the stop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Read one HTTP request head off `sock` (closing with it unread would RST).
    async fn read_head(sock: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match sock.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    /// A complete file must not be deleted and restarted when the server
    /// answers our EOF resume offset with 416.
    #[tokio::test]
    async fn complete_file_is_kept() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut sock, _) = listener.accept().await.unwrap();
            read_head(&mut sock).await;
            sock.write_all(
                b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */5\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
            let _ = sock.shutdown().await;
        });
        let client = reqwest::Client::new();
        let dir = std::env::temp_dir().join(format!("werk-done-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("f.bin");
        std::fs::write(&dest, b"hello").unwrap();
        let out = download_to(&client, &format!("http://{addr}/f.bin"), &dest, &|| false, |_| {}, None)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An offset past EOF is a stale partial: drop it and fetch again.
    #[tokio::test]
    async fn oversized_partial_restarts() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut sock, _) = listener.accept().await.unwrap();
            read_head(&mut sock).await;
            sock.write_all(
                b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */3\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
            let _ = sock.shutdown().await;
            drop(sock);
            let (mut sock, _) = listener.accept().await.unwrap();
            read_head(&mut sock).await;
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc")
                .await
                .unwrap();
            let _ = sock.shutdown().await;
        });
        let client = reqwest::Client::new();
        let dir = std::env::temp_dir().join(format!("werk-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("f.bin");
        std::fs::write(&dest, b"hello").unwrap();
        let out = download_to(&client, &format!("http://{addr}/f.bin"), &dest, &|| false, |_| {}, None)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"abc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `sha256("hello")`, used by the pin tests below.
    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[tokio::test]
    async fn verifies_sha256_while_streaming() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                read_head(&mut sock).await;
                sock.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .await
                .unwrap();
                let _ = sock.shutdown().await;
            }
        });
        let client = reqwest::Client::new();
        let dir = std::env::temp_dir().join(format!("werk-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("f.bin");
        let url = format!("http://{addr}/f.bin");
        // The right pin passes.
        let out = download_to(&client, &url, &dest, &|| false, |_| {}, Some(HELLO_SHA256))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        // A wrong pin fails and removes the file so a retry starts clean.
        std::fs::remove_file(&dest).unwrap();
        let err = download_to(&client, &url, &dest, &|| false, |_| {}, Some("00"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Checksum mismatch"), "{err}");
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn verifies_resumed_prefix_against_the_pin() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut sock, _) = listener.accept().await.unwrap();
            read_head(&mut sock).await;
            sock.write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 3-4/5\r\nContent-Length: 2\r\nConnection: close\r\n\r\nlo",
            )
            .await
            .unwrap();
            let _ = sock.shutdown().await;
        });
        let client = reqwest::Client::new();
        let dir = std::env::temp_dir().join(format!("werk-pin-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("f.bin");
        std::fs::write(&dest, b"hel").unwrap();
        let out = download_to(
            &client,
            &format!("http://{addr}/f.bin"),
            &dest,
            &|| false,
            |_| {},
            Some(HELLO_SHA256),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
