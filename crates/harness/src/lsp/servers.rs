//! Language-server specs built from settings, PATH probing, and root markers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A spawnable stdio language server: program + argv, the extensions it owns,
/// and the marker files that identify its workspace root.
pub struct ServerSpec {
    pub name: String,
    pub command: Vec<String>,
    /// (lowercase extension, LSP language id)
    pub files: Vec<(String, String)>,
    pub roots: Vec<String>,
}

impl ServerSpec {
    /// Build from a settings row; `None` when unusable (no program or files).
    pub fn build(
        name: &str,
        command_line: &str,
        extensions: &[String],
        language: Option<&str>,
        roots: &[String],
    ) -> Option<ServerSpec> {
        let name = name.trim().to_string();
        let command = split_command_line(command_line);
        let language = language
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string);
        let files: Vec<(String, String)> = extensions
            .iter()
            .filter_map(|raw| {
                let extension = normalize_ext(raw)?;
                let language = language.clone().unwrap_or_else(|| extension.clone());
                Some((extension, language))
            })
            .collect();
        if name.is_empty() || command.is_empty() || files.is_empty() {
            return None;
        }
        Some(ServerSpec {
            name,
            command,
            files,
            roots: roots
                .iter()
                .filter_map(|r| {
                    let r = r.trim();
                    (!r.is_empty()).then(|| r.to_string())
                })
                .collect(),
        })
    }
}

/// Server owning `path`'s extension, first match wins.
pub fn spec_for<'a>(servers: &'a [Arc<ServerSpec>], path: &Path) -> Option<&'a Arc<ServerSpec>> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    servers
        .iter()
        .find(|s| s.files.iter().any(|(e, _)| *e == ext))
}

pub fn language_id<'a>(spec: &'a ServerSpec, path: &Path) -> &'a str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    spec.files
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, id)| id.as_str())
        .unwrap_or("plaintext")
}

/// Nearest ancestor holding a root marker, never above `floor`.
pub fn root_for(spec: &ServerSpec, file: &Path, floor: &Path) -> PathBuf {
    let mut dir = file.parent().unwrap_or(floor).to_path_buf();
    loop {
        if spec.roots.iter().any(|m| dir.join(m).exists()) {
            return dir;
        }
        if dir == floor {
            break;
        }
        match dir.parent() {
            Some(parent) if parent.starts_with(floor) => dir = parent.to_path_buf(),
            _ => break,
        }
    }
    floor.to_path_buf()
}

/// Resolve the program token: an explicit path wins, else probe PATH.
pub fn resolve_program(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() || program.contains(['/', '\\']) {
        return path.is_file().then(|| path.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
    find_in_dirs(&dirs, &[program], &exts())
}

fn exts() -> Vec<String> {
    #[cfg(windows)]
    {
        std::env::var("PATHEXT")
            .map(|v| {
                v.split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_else(|_| vec![".exe".into(), ".cmd".into(), ".bat".into(), ".com".into()])
    }
    #[cfg(not(windows))]
    {
        vec![String::new()]
    }
}

pub(crate) fn find_in_dirs(dirs: &[PathBuf], names: &[&str], exts: &[String]) -> Option<PathBuf> {
    for dir in dirs {
        for name in names {
            for ext in exts {
                let candidate = dir.join(format!("{name}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn normalize_ext(raw: &str) -> Option<String> {
    let ext = raw.trim().trim_start_matches('.').to_ascii_lowercase();
    (!ext.is_empty() && !ext.contains(['/', '\\', ' '])).then_some(ext)
}

/// Split a command line on whitespace; quotes group and are dropped.
fn split_command_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in line.trim().chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c.is_whitespace() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            None => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("werk-lsp-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spec() -> ServerSpec {
        ServerSpec::build(
            "rust-analyzer",
            "rust-analyzer",
            &["rs".to_string()],
            Some("rust"),
            &["Cargo.toml".to_string()],
        )
        .unwrap()
    }

    #[test]
    fn builds_and_matches_by_extension() {
        let rust = Arc::new(spec());
        let servers = vec![rust];
        assert_eq!(
            spec_for(&servers, Path::new("a.RS")).unwrap().name,
            "rust-analyzer"
        );
        assert!(spec_for(&servers, Path::new("a.txt")).is_none());
        assert_eq!(language_id(&servers[0], Path::new("a.rs")), "rust");
        assert_eq!(language_id(&servers[0], Path::new("a.txt")), "plaintext");

        // No language id -> the extension itself is the id.
        let clangd = ServerSpec::build(
            "clangd",
            "clangd",
            &[".C".to_string(), "cpp".to_string()],
            None,
            &[],
        )
        .unwrap();
        assert_eq!(clangd.files[0], ("c".to_string(), "c".to_string()));
        assert_eq!(clangd.files[1], ("cpp".to_string(), "cpp".to_string()));

        assert!(ServerSpec::build("x", "", &["rs".to_string()], None, &[]).is_none());
        assert!(ServerSpec::build("x", "prog", &[], None, &[]).is_none());
        assert!(ServerSpec::build("x", "prog", &[".".to_string()], None, &[]).is_none());
    }

    #[test]
    fn command_lines_split_with_quotes() {
        assert_eq!(split_command_line("typescript-language-server --stdio"), [
            "typescript-language-server",
            "--stdio"
        ]);
        assert_eq!(
            split_command_line(r#""C:\Program Files\godot\godot.exe" --headless --lsp"#),
            ["C:\\Program Files\\godot\\godot.exe", "--headless", "--lsp"]
        );
        assert_eq!(split_command_line("  npx   -y 'some pkg'  "), ["npx", "-y", "some pkg"]);
        assert!(split_command_line("   ").is_empty());
    }

    #[test]
    fn root_walks_to_nearest_marker_but_not_above_floor() {
        let floor = temp_dir("root");
        let crate_dir = floor.join("crates").join("app");
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(crate_dir.join("Cargo.toml"), "[package]").unwrap();
        let spec = spec();
        let file = crate_dir.join("src").join("main.rs");
        assert_eq!(root_for(&spec, &file, &floor), crate_dir);
        // Marker above the floor is ignored.
        std::fs::remove_file(crate_dir.join("Cargo.toml")).unwrap();
        std::fs::write(floor.join("Cargo.toml"), "[workspace]").unwrap();
        assert_eq!(root_for(&spec, &file, &floor), floor);
        let _ = std::fs::remove_dir_all(&floor);
    }

    #[test]
    fn program_resolution_accepts_paths_and_path_lookups() {
        let dir = temp_dir("bin");
        let exe = if cfg!(windows) { "probe-lsp.exe" } else { "probe-lsp" };
        let file = dir.join(exe);
        std::fs::write(&file, "").unwrap();
        assert_eq!(resolve_program(file.to_str().unwrap()).as_deref(), Some(file.as_path()));
        assert!(resolve_program(dir.join("missing.exe").to_str().unwrap()).is_none());

        let exts = if cfg!(windows) { vec![".exe".to_string()] } else { vec![String::new()] };
        assert_eq!(
            find_in_dirs(std::slice::from_ref(&dir), &["probe-lsp"], &exts).as_deref(),
            Some(file.as_path())
        );
        assert!(find_in_dirs(std::slice::from_ref(&dir), &["clangd"], &exts).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
