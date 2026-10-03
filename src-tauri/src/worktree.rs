//! Git worktrees for chat projects: one branch per folder.
//! All git access goes through the CLI; parsing is pure and tested.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct WorktreeDto {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    pub bare: bool,
    pub main: bool,
}

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    crate::hidden::command("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("git is not installed")
}

pub fn is_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Sibling folder for a branch: `<rootname>-<branch>` next to the project.
pub fn worktree_path(root: &Path, branch: &str) -> PathBuf {
    let stem = root.file_name().and_then(|n| n.to_str()).unwrap_or("work");
    let safe: String = branch
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    root.parent().unwrap_or(Path::new(".")).join(format!("{stem}-{safe}"))
}

pub fn check_branch(branch: &str) -> Result<()> {
    if branch.trim().is_empty() {
        anyhow::bail!("Branch name is empty");
    }
    if branch.contains("..") || branch.chars().any(|c| "~^:?*[\\".contains(c)) {
        anyhow::bail!("Unsafe branch name: {branch}");
    }
    Ok(())
}

pub fn list(root: &Path) -> Result<Vec<WorktreeDto>> {
    let out = git(root, &["worktree", "list", "--porcelain"])?;
    if !out.status.success() {
        anyhow::bail!("Not a git repository: {}", root.display());
    }
    Ok(parse_porcelain(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_porcelain(text: &str) -> Vec<WorktreeDto> {
    let mut out = Vec::new();
    let mut cur: Option<WorktreeDto> = None;
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(w) = cur.take() {
                out.push(w);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(w) = cur.take() {
                out.push(w);
            }
            cur = Some(WorktreeDto {
                path: path.to_string(),
                branch: None,
                head: None,
                bare: false,
                main: out.is_empty(),
            });
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            if let Some(w) = cur.as_mut() {
                w.head = Some(head.to_string());
            }
        } else if let Some(branch) = line.strip_prefix("branch ") {
            if let Some(w) = cur.as_mut() {
                w.branch = branch.strip_prefix("refs/heads/").unwrap_or(branch).to_string().into();
            }
        } else if line == "bare" {
            if let Some(w) = cur.as_mut() {
                w.bare = true;
            }
        } else if line == "detached" {
            if let Some(w) = cur.as_mut() {
                w.branch = None;
            }
        }
    }
    out
}

pub fn add(root: &Path, branch: &str) -> Result<PathBuf> {
    check_branch(branch)?;
    let path = worktree_path(root, branch);
    if path.exists() {
        anyhow::bail!("Already exists: {}", path.display());
    }
    let out = git(root, &["worktree", "add", &path.to_string_lossy(), "-b", branch])?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(path)
}

pub fn remove(root: &Path, path: &str, force: bool) -> Result<()> {
    if path.contains("..") {
        anyhow::bail!("Unsafe path: {path}");
    }
    let mut args = vec!["worktree", "remove", path];
    if force {
        args.push("--force");
    }
    let out = git(root, &args)?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_parses_main_branches_and_detached() {
        let text = "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\nworktree /repo-feat\nHEAD def456\nbranch refs/heads/feat\n\nworktree /repo-det\nHEAD 789aaa\ndetached\n\n";
        let list = parse_porcelain(text);
        assert_eq!(list.len(), 3);
        assert!(list[0].main);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert!(!list[1].main);
        assert_eq!(list[1].branch.as_deref(), Some("feat"));
        assert_eq!(list[2].branch, None);
        assert_eq!(list[2].head.as_deref(), Some("789aaa"));
    }

    #[test]
    fn branch_names_checked() {
        assert!(check_branch("feat-x").is_ok());
        assert!(check_branch("").is_err());
        assert!(check_branch("../evil").is_err());
        assert!(check_branch("a*b").is_err());
    }

    #[test]
    fn sibling_path_derives() {
        let p = worktree_path(Path::new("/work/proj"), "feat");
        assert_eq!(p, PathBuf::from("/work/proj-feat"));
        let p = worktree_path(Path::new("/work/proj"), "a/b");
        assert_eq!(p, PathBuf::from("/work/proj-a-b"));
    }
}
