//! Minimal `file://` URI conversion (no url dependency, exact paths).

use std::path::{Path, PathBuf};

fn keep(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// Absolute path -> `file:///...`, percent-encoding anything unusual.
pub fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let raw = raw.strip_prefix('/').unwrap_or(&raw);
    let mut out = String::with_capacity(raw.len() + 8);
    out.push_str("file:///");
    for &b in raw.as_bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// `file:///...` -> absolute path; `None` for non-file URIs.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let decoded = percent_decode(uri.strip_prefix("file://")?)?;
    #[cfg(windows)]
    {
        let s = decoded.strip_prefix('/').unwrap_or(&decoded);
        Some(PathBuf::from(s.replace('/', "\\")))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from(decoded))
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let v = u8::from_str_radix(std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?, 16).ok()?;
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_odd_paths() {
        let path = std::env::temp_dir().join("werk lsp ü#1").join("a b.rs");
        assert_eq!(uri_to_path(&path_to_uri(&path)).as_deref(), Some(path.as_path()));
        assert!(path_to_uri(&path).starts_with("file:///"));
        assert!(path_to_uri(&path).contains("%20"));
        assert!(path_to_uri(&path).contains("%23"));
    }

    #[test]
    fn rejects_non_file_uris_and_bad_escapes() {
        assert_eq!(uri_to_path("https://example.com/x"), None);
        assert_eq!(uri_to_path("file:///x/%zz"), None);
    }
}
