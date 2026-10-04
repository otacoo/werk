//! Project jail: every tool path resolves under one root.
//! Approvals gate whether a tool runs; the jail owns where it may reach.

use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Result};

/// Directories the agent may read but never write: its own tool and skill
/// definitions, so a model can never add a tool to its own next run.
const PROTECTED_WRITE: &[&str] = &[".werk/plugins", ".werk/skills"];

/// `AGENTS.md` (any case) and `.agent*` names: the agent's own instruction
/// files, hidden from every tool when the user turns them off.
fn is_agent_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            let lower = n.to_ascii_lowercase();
            lower == "agents.md" || lower.starts_with(".agent")
        })
        .unwrap_or(false)
}

/// Sandboxed root plus extra read-only roots (never writable).
#[derive(Debug, Clone)]
pub struct PathJail {
    root: PathBuf,
    extra_read: Vec<PathBuf>,
    /// Additional read/write roots (the assistant's configured folders).
    extra_write: Vec<PathBuf>,
    hide_agent_files: bool,
    sensitive: Option<std::sync::Arc<crate::sensitive::SensitivePolicy>>,
}

impl PathJail {
    pub fn new(root: &Path, extra_read: &[PathBuf]) -> Result<Self> {
        let root = strip_verbatim(root.canonicalize().map_err(|e| {
            anyhow::anyhow!("Project root {} unreadable: {e}", root.display())
        })?);
        if !root.is_dir() {
            bail!("Project root {} is not a directory", root.display());
        }
        Ok(Self {
            root,
            // Canonicalized like the root, so symlinked spellings match.
            extra_read: extra_read
                .iter()
                .filter_map(|p| p.canonicalize().ok())
                .map(strip_verbatim)
                .collect(),
            extra_write: Vec::new(),
            hide_agent_files: false,
            sensitive: None,
        })
    }

    /// Additional roots the jail may read and write. Relative paths still
    /// resolve under the primary root; absolute ones may land in any root.
    pub fn with_write_roots(mut self, roots: &[PathBuf]) -> Self {
        self.extra_write = roots
            .iter()
            .filter_map(|p| p.canonicalize().ok())
            .map(strip_verbatim)
            .collect();
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Hide `AGENTS.md`/`.agent*` files from every tool path when set.
    pub fn hide_agent_files(mut self, hide: bool) -> Self {
        self.hide_agent_files = hide;
        self
    }

    /// True when this path is an agent file the jail currently hides.
    pub fn hides(&self, path: &Path) -> bool {
        self.hide_agent_files && is_agent_file(path)
    }

    /// Attach the sensitive-file policy (built-ins + `.gitignore` + user list).
    pub fn shield(mut self, policy: std::sync::Arc<crate::sensitive::SensitivePolicy>) -> Self {
        self.sensitive = Some(policy);
        self
    }

    /// True when the sensitive-file policy shields this path.
    pub fn shielded(&self, path: &Path) -> bool {
        self.sensitive.as_ref().map(|p| p.shielded(path)).unwrap_or(false)
    }

    /// Lexical join: `..` above the root is an error, never an escape.
    fn join(&self, rel: &str) -> Result<PathBuf> {
        let mut out = self.root.clone();
        for comp in Path::new(rel).components() {
            match comp {
                Component::Prefix(_) | Component::RootDir => {
                    bail!("Absolute paths are not allowed: {rel}")
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() || out != self.root && !out.starts_with(&self.root) {
                        bail!("Path escapes the project: {rel}")
                    }
                }
                Component::Normal(part) => out.push(part),
            }
        }
        if out != self.root && !out.starts_with(&self.root) {
            bail!("Path escapes the project: {rel}")
        }
        Ok(out)
    }

    pub fn check_read(&self, rel: &str) -> Result<PathBuf> {
        if rel.trim().is_empty() {
            bail!("Path must not be empty");
        }
        // Absolute paths are accepted inside the root (or an extra root here).
        let p = Path::new(rel);
        let p = if p.is_absolute() {
            self.absolute(p, false)?
        } else {
            self.join(rel)?
        };
        if self.hides(&p) {
            bail!("Agent files are hidden (Agent tab): {rel}");
        }
        if self.shielded(&p) {
            bail!("Path is shielded by the sensitive-file policy: {rel}");
        }
        Ok(p)
    }

    pub fn check_write(&self, rel: &str) -> Result<PathBuf> {
        let raw = Path::new(rel);
        let p = if raw.is_absolute() {
            self.absolute(raw, true)?
        } else {
            self.join(rel)?
        };
        if self.hides(&p) {
            bail!("Agent files are hidden (Agent tab): {rel}");
        }
        if self.shielded(&p) {
            bail!("Path is shielded by the sensitive-file policy: {rel}");
        }
        if self.extra_read.iter().any(|r| under(r, &p)) {
            bail!("Path is read-only: {rel}");
        }
        for r in std::iter::once(&self.root).chain(self.extra_write.iter()) {
            for protected in PROTECTED_WRITE {
                if under(&r.join(protected), &p) {
                    bail!(
                        "Path is protected: tool and skill definitions are user-authored ({rel})"
                    );
                }
            }
        }
        Ok(p)
    }

    /// Absolute input: allowed when it lands inside a writable root, or a
    /// read-only root on reads. Symlinks resolve against the filesystem so
    /// macOS `/var` vs `/private/var` spellings compare equal; `..` may not
    /// climb out.
    fn absolute(&self, path: &Path, writable: bool) -> Result<PathBuf> {
        let clean = canonical_spelling(path);
        let writable_hit =
            under(&self.root, &clean) || self.extra_write.iter().any(|r| under(r, &clean));
        if writable_hit {
            return Ok(clean);
        }
        if !writable && self.extra_read.iter().any(|r| under(r, &clean)) {
            return Ok(clean);
        }
        bail!("Path is outside the allowed folders: {}", path.display())
    }
}

/// Resolve symlinks in the deepest existing ancestor, keeping the rest of the
/// path lexical. macOS temp dirs live under `/var` (a symlink to
/// `/private/var`), so a raw model path would otherwise miss the jail root.
fn canonical_spelling(path: &Path) -> PathBuf {
    if let Ok(resolved) = path.canonicalize() {
        return strip_verbatim(resolved);
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut probe = path.to_path_buf();
    while let Some(name) = probe.file_name().map(|n| n.to_os_string()) {
        tail.push(name);
        if !probe.pop() {
            break;
        }
        if let Ok(base) = probe.canonicalize() {
            let mut out = strip_verbatim(base);
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
    }
    strip_verbatim(clean_absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// Drop the Windows verbatim prefix `\\?\` that `canonicalize` adds, so jail
/// paths compare (and render) like the ones the model writes.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

/// True when `path` is `root` or below it; component-wise, case-insensitive
/// on Windows so `e:\TEST\x` matches `E:\test`.
fn under(root: &Path, path: &Path) -> bool {
    let mut root = root.components();
    let mut path = path.components();
    loop {
        match (root.next(), path.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(a), Some(b)) => {
                #[cfg(windows)]
                let equal = a
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy());
                #[cfg(not(windows))]
                let equal = a == b;
                if !equal {
                    return false;
                }
            }
        }
    }
}

/// Lexically normalize an absolute path without touching the filesystem.
fn clean_absolute(path: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    bail!("Path escapes its root: {}", path.display());
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jail(label: &str) -> (PathBuf, PathBuf, PathJail) {
        let dir = std::env::temp_dir().join(format!("werk-jail-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // Canonicalized and de-verbatimed, matching what the jail stores.
        let canon = strip_verbatim(dir.canonicalize().unwrap());
        let jail = PathJail::new(&dir, &[]).unwrap();
        (dir, canon, jail)
    }

    #[test]
    fn read_write_resolve_inside_root() {
        let (dir, canon, jail) = jail("rw");
        assert_eq!(jail.check_read("sub").unwrap(), canon.join("sub"));
        assert_eq!(jail.check_write("new.txt").unwrap(), canon.join("new.txt"));
        assert_eq!(jail.check_read("sub/../new.txt").unwrap(), canon.join("new.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn escapes_rejected() {
        let (dir, _, jail) = jail("esc");
        assert!(jail.check_read("../evil").is_err());
        assert!(jail.check_read("sub/../../evil").is_err());
        assert!(jail.check_write("../evil").is_err());
        assert!(jail.check_read("/abs/path").is_err());
        assert!(jail.check_read("").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_write_roots_are_writable() {
        let (dir, canon, jail) = jail("multi");
        let second = std::env::temp_dir().join(format!("werk-jail-second-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&second);
        std::fs::create_dir_all(&second).unwrap();
        let jail = jail.with_write_roots(&[second.clone()]);
        let second_canon = strip_verbatim(second.canonicalize().unwrap());
        let target = second_canon.join("x.txt").to_string_lossy().to_string();
        assert_eq!(jail.check_write(&target).unwrap(), second_canon.join("x.txt"));
        assert_eq!(jail.check_read(&target).unwrap(), second_canon.join("x.txt"));
        // Relative paths still resolve under the primary root.
        assert_eq!(jail.check_write("rel.txt").unwrap(), canon.join("rel.txt"));
        // Outside every root is rejected.
        let outside = std::env::temp_dir()
            .join(format!("werk-jail-outside-{}", std::process::id()))
            .join("nope.txt")
            .to_string_lossy()
            .to_string();
        assert!(jail.check_write(&outside).is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&second);
    }

    #[test]
    fn absolute_paths_inside_root_are_accepted() {
        let (dir, canon, jail) = jail("abs");
        let target = canon.join("sub").join("a.txt");
        let raw = target.to_string_lossy().to_string();
        assert_eq!(jail.check_write(&raw).unwrap(), target);
        assert_eq!(jail.check_read(&raw).unwrap(), target);
        // The plain (non-canonicalized) spelling a model would send works too.
        let plain = dir.join("plain.txt").to_string_lossy().to_string();
        assert!(jail.check_write(&plain).is_ok(), "{plain}");
        // `..` that still lands inside the root is fine.
        let inside = canon.join("sub").join("..").join("b.txt");
        assert_eq!(
            jail.check_write(&inside.to_string_lossy()).unwrap(),
            canon.join("b.txt")
        );
        // Outside the root stays rejected.
        let outside = std::env::temp_dir().join("werk-jail-outside.txt");
        assert!(jail.check_write(&outside.to_string_lossy()).is_err());
        assert!(jail.check_read(&outside.to_string_lossy()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn absolute_paths_ignore_case_on_windows() {
        let (dir, canon, jail) = jail("case");
        let shouty = format!("{}\\SUB\\A.TXT", canon.to_string_lossy().to_uppercase());
        assert!(jail.check_write(&shouty).is_ok(), "{shouty}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sensitive_policy_blocks_reads_and_writes() {
        let (dir, canon, jail) = jail("shield");
        let policy = std::sync::Arc::new(crate::sensitive::SensitivePolicy::build(&canon, &[], &[]));
        let jail = jail.shield(policy);
        assert!(jail.check_read(".env").is_err());
        assert!(jail.check_write("new.pem").is_err());
        assert!(jail.check_read("sub/notes.txt").is_ok());
        assert!(jail.shielded(&canon.join(".ssh").join("id_ed25519")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_files_can_be_hidden() {
        let (dir, _, jail) = jail("agentfiles");
        let hidden = jail.clone().hide_agent_files(true);
        assert!(hidden.check_read("AGENTS.md").is_err());
        assert!(hidden.check_read("sub/.agent").is_err());
        assert!(hidden.check_write("AGENTS.md").is_err());
        assert!(hidden.hides(&dir.join(".agentrc")));
        assert!(!hidden.hides(&dir.join("src/main.rs")));
        // Visible by default, and other files are unaffected.
        assert!(jail.check_read("AGENTS.md").is_ok());
        assert!(hidden.check_read("src/notes.md").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tool_and_skill_definitions_are_write_protected() {
        let (dir, _, jail) = jail("protect");
        assert!(jail.check_write(".werk/plugins/x/plugin.json").is_err());
        assert!(jail.check_write(".werk/skills/s.md").is_err());
        assert!(jail.check_write(".werk/plugins").is_err());
        // Reading them is fine, and the rest of `.werk` stays writable.
        assert!(jail.check_read(".werk/plugins/x/plugin.json").is_ok());
        assert!(jail.check_write(".werk/MEMORY.md").is_ok());
        assert!(jail.check_write(".werk/notes.txt").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_roots_readable_never_writable() {
        let (dir, _, _) = jail("ro");
        let ro = std::env::temp_dir().join(format!("werk-jail-rox-{}", std::process::id()));
        std::fs::create_dir_all(&ro).unwrap();
        let ro_canon = strip_verbatim(ro.canonicalize().unwrap());
        let jail = PathJail::new(&dir, &[ro.clone()]).unwrap();
        let abs = ro.join("f.txt").to_string_lossy().to_string();
        assert_eq!(jail.check_read(&abs).unwrap(), ro_canon.join("f.txt"));
        assert!(jail.check_write(&abs).is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&ro);
    }
}
