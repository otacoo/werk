//! Facts injected into the system prompt at run start: date/time, git state,
//! and a shallow top-level listing of the project.

use std::path::Path;

const LISTING_CAP: usize = 40;
const LISTING_SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
    ".gradle",
    ".idea",
    ".werk",
];

/// Facts injected into the system prompt at run start: date, git state, and a
/// shallow top-level listing. The project path itself is already in the prompt.
pub fn run_context_block(root: &Path) -> String {
    let mut out = String::from("\n\nRun context:");
    out.push_str(&format!("\n- Date: {} {} UTC", harness::memory::today_iso(), utc_clock()));
    if let Some(git) = git_summary(root) {
        out.push_str(&format!("\n- Git: {git}"));
    }
    let listing = top_level_listing(root);
    if !listing.is_empty() {
        out.push_str(&format!("\n- Top level: {listing}"));
    }
    out
}

fn utc_clock() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{:02}:{:02}", (secs % 86_400) / 3600, (secs % 3600) / 60)
}

/// Branch, short HEAD, and dirty count; None outside a repository.
fn git_summary(root: &Path) -> Option<String> {
    let run = |args: &[&str]| -> Option<String> {
        let out = crate::hidden::command("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let head = run(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let dirty = run(&["status", "--porcelain"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    Some(if dirty == 0 {
        format!("branch {branch}, HEAD {head}, clean")
    } else {
        format!("branch {branch}, HEAD {head}, {dirty} uncommitted file(s)")
    })
}

fn top_level_listing(root: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(root) else {
        return String::new();
    };
    let mut names: Vec<String> = Vec::new();
    let mut truncated = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if LISTING_SKIP.contains(&name.as_str()) {
            continue;
        }
        if names.len() >= LISTING_CAP {
            truncated = true;
            break;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        names.push(if is_dir { format!("{name}/") } else { name });
    }
    names.sort();
    let mut out = names.join(", ");
    if truncated {
        out.push_str(", …");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("werk-ctx-{tag}-{}", std::process::id()))
    }

    #[test]
    fn run_context_lists_the_project() {
        let dir = temp_dir("list");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "x").unwrap();
        let block = run_context_block(&dir);
        assert!(block.contains("Run context"), "{block}");
        assert!(block.contains("UTC"), "{block}");
        assert!(block.contains("src/"), "{block}");
        assert!(block.contains("Cargo.toml"), "{block}");
        assert!(!block.contains("node_modules"), "{block}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listing_is_capped_and_sorted() {
        let dir = temp_dir("cap");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..LISTING_CAP + 5 {
            std::fs::write(dir.join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let listing = top_level_listing(&dir);
        assert!(listing.ends_with('…'), "{listing}");
        assert_eq!(listing.matches("f").count(), LISTING_CAP);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
