//! Sensitive-file policy: credential paths never reach the model. Built-in
//! patterns, the project's `.gitignore`, and a user list are merged
//! tighten-only — a repo rule can never un-shield a built-in — with an
//! explicit allow list for exceptions. Tool output is also scrubbed for
//! key-shaped strings before it reaches the model or the session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// File-name globs always shielded (credentials and key material).
const NAME_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa*",
    "id_ed25519*",
    ".npmrc",
    ".netrc",
    ".pgpass",
    "credentials",
    "credentials.*",
    "*.sqlite",
    "*.sqlite3",
];

/// Directory names always shielded (any path component).
const DIR_NAMES: &[&str] = &[".ssh", ".gnupg", ".aws"];

#[derive(Debug)]
pub struct SensitivePolicy {
    root: PathBuf,
    name_globs: Vec<glob::Pattern>,
    user_globs: Vec<glob::Pattern>,
    allow_globs: Vec<glob::Pattern>,
    /// Per-directory `.gitignore` matchers, loaded lazily (`None` = no file).
    /// The cache also keeps compiled patterns reusable across large trees.
    gitignores: Mutex<HashMap<PathBuf, Option<Arc<ignore::gitignore::Gitignore>>>>,
}

fn compile(patterns: &[String]) -> Vec<glob::Pattern> {
    patterns
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .filter_map(|p| glob::Pattern::new(p).ok())
        .collect()
}

fn matches_globs(globs: &[glob::Pattern], name: &str, rel: &str) -> bool {
    globs.iter().any(|g| g.matches(name) || (!rel.is_empty() && g.matches(rel)))
}

impl SensitivePolicy {
    pub fn build(root: &Path, user: &[String], allow: &[String]) -> Self {
        Self {
            root: root.to_path_buf(),
            name_globs: NAME_PATTERNS.iter().filter_map(|p| glob::Pattern::new(p).ok()).collect(),
            user_globs: compile(user),
            allow_globs: compile(allow),
            gitignores: Mutex::new(HashMap::new()),
        }
    }

    /// The `.gitignore` matcher for `dir`, loaded and cached on first use.
    fn gitignore_for(&self, dir: &Path) -> Option<Arc<ignore::gitignore::Gitignore>> {
        if let Some(hit) = self.gitignores.lock().unwrap().get(dir) {
            return hit.clone();
        }
        let file = dir.join(".gitignore");
        let loaded = if file.is_file() {
            let mut builder = ignore::gitignore::GitignoreBuilder::new(dir);
            let _ = builder.add(&file);
            builder.build().ok().map(Arc::new)
        } else {
            None
        };
        self.gitignores
            .lock()
            .unwrap()
            .insert(dir.to_path_buf(), loaded.clone());
        loaded
    }

    /// Deepest-first `.gitignore` decision for `path`: `Some(true)` when
    /// ignored, `Some(false)` when a nested negation explicitly un-ignores it,
    /// `None` when no rule matched.
    fn gitignore_match(&self, path: &Path, is_dir: bool) -> Option<bool> {
        let mut dir = path.parent()?;
        while dir.starts_with(&self.root) {
            if let Some(gi) = self.gitignore_for(dir) {
                match gi.matched_path_or_any_parents(path, is_dir) {
                    ignore::Match::Ignore(_) => return Some(true),
                    ignore::Match::Whitelist(_) => return Some(false),
                    ignore::Match::None => {}
                }
            }
            if dir == self.root {
                break;
            }
            dir = dir.parent()?;
        }
        None
    }

    /// True when the path is shielded by built-ins, the user list, or a
    /// `.gitignore` (nested files included). The allow list exempts;
    /// built-ins and the user list always win over `.gitignore` negations.
    pub fn shielded(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let rel = path
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if matches_globs(&self.allow_globs, name, &rel) {
            return false;
        }
        if matches_globs(&self.name_globs, name, "") {
            return true;
        }
        if rel
            .split('/')
            .any(|part| DIR_NAMES.iter().any(|d| part.eq_ignore_ascii_case(d)))
        {
            return true;
        }
        if matches_globs(&self.user_globs, name, &rel) {
            return true;
        }
        matches!(self.gitignore_match(path, path.is_dir()), Some(true))
    }
}

/// Key-shaped strings scrubbed from tool output before it reaches the model
/// or the session. The generic form keeps its label so context survives.
pub fn redact_secrets(text: &str) -> String {
    static RES: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [
            r"sk-[A-Za-z0-9_\-]{16,}",
            r"ghp_[A-Za-z0-9]{20,}",
            r"github_pat_[A-Za-z0-9_]{20,}",
            r"AKIA[0-9A-Z]{16}",
            r"xox[baprs]-[A-Za-z0-9\-]{10,}",
            r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{6,}",
            r#"(?i)(api[_-]?key|secret|token|password)["']?\s*[:=]\s*["']?[A-Za-z0-9_\-]{12,}"#,
        ]
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect()
    });
    let mut out = text.to_string();
    for re in res {
        out = re
            .replace_all(&out, |caps: &regex::Captures| match caps[0].find([':', '=']) {
                Some(i) => format!("{} [redacted]", &caps[0][..=i]),
                None => "[redacted]".to_string(),
            })
            .into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(tag: &str, gitignore: Option<&str>, user: &[&str], allow: &[&str]) -> (PathBuf, SensitivePolicy) {
        let root = std::env::temp_dir().join(format!("werk-shield-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        if let Some(text) = gitignore {
            std::fs::write(root.join(".gitignore"), text).unwrap();
        }
        let user: Vec<String> = user.iter().map(|s| s.to_string()).collect();
        let allow: Vec<String> = allow.iter().map(|s| s.to_string()).collect();
        let policy = SensitivePolicy::build(&root, &user, &allow);
        (root, policy)
    }

    #[test]
    fn builtin_names_and_dirs_are_shielded() {
        let (root, p) = policy("builtin", None, &[], &[]);
        for path in [".env", ".env.local", "server.pem", "private.key", "id_rsa", "app.sqlite"] {
            assert!(p.shielded(&root.join(path)), "{path}");
        }
        assert!(p.shielded(&root.join(".ssh").join("id_ed25519")));
        assert!(p.shielded(&root.join("nested").join(".aws").join("credentials")));
        assert!(!p.shielded(&root.join("src").join("main.rs")));
        assert!(!p.shielded(&root.join("README.md")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gitignore_is_respected_but_negations_cannot_unshield_builtins() {
        let (root, p) = policy("gitignore", Some("secrets/\n*.log\n!.env\n"), &[], &[]);
        assert!(p.shielded(&root.join("secrets").join("note.txt")));
        assert!(p.shielded(&root.join("build.log")));
        // A repo negation must not un-shield a built-in pattern.
        assert!(p.shielded(&root.join(".env")));
        assert!(!p.shielded(&root.join("src").join("lib.rs")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nested_gitignores_scope_to_their_directory() {
        let root = std::env::temp_dir().join(format!("werk-shield-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub").join("build")).unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(root.join("sub").join(".gitignore"), "build/\n!keep.log\n").unwrap();
        let p = SensitivePolicy::build(&root, &[], &[]);
        assert!(p.shielded(&root.join("top.log")));
        assert!(p.shielded(&root.join("sub").join("note.log")));
        assert!(p.shielded(&root.join("sub").join("build").join("x.txt")));
        // The nested rule does not apply outside its directory.
        assert!(!p.shielded(&root.join("build").join("x.txt")));
        // A nested negation overrides the root's `*.log`.
        assert!(!p.shielded(&root.join("sub").join("keep.log")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn user_patterns_and_allow_list() {
        let (root, p) = policy("user", None, &["*.secret", "config/private/*"], &["test.pem"]);
        assert!(p.shielded(&root.join("a.secret")));
        assert!(p.shielded(&root.join("config").join("private").join("x.txt")));
        // The allow list exempts even a built-in match.
        assert!(!p.shielded(&root.join("test.pem")));
        assert!(p.shielded(&root.join("prod.pem")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn redaction_hides_key_shapes_and_keeps_text() {
        let out = redact_secrets("key sk-abcdefghijklmnopqrstuvwx done");
        assert!(!out.contains("sk-abcdefghijklmnopqrstuvwx"), "{out}");
        assert!(out.contains("[redacted]"), "{out}");
        let out = redact_secrets(r#"{"api_key": "abcdefghijklmnop1234"}"#);
        assert!(out.contains("api_key") && !out.contains("abcdefghijklmnop1234"), "{out}");
        let out = redact_secrets("plain words and a path src/main.rs");
        assert_eq!(out, "plain words and a path src/main.rs");
    }
}
