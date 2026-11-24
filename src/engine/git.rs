//! The git operations the engine performs. Agents never run these.

use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Runs git and returns trimmed stdout. A non-zero exit is an error.
pub fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

pub fn head(worktree: &Path) -> Result<String> {
    git(worktree, &["rev-parse", "HEAD"])
}

/// Uncommitted and untracked paths, one per line. Empty when clean.
pub fn dirty(worktree: &Path) -> Result<String> {
    git(worktree, &["status", "--porcelain"])
}

/// A fresh worktree on a new branch at `base_ref`. An older worktree at the
/// same path is the engine's own leftover from an earlier run and is replaced.
pub fn fresh_worktree(repo: &Path, path: &Path, branch: &str, base_ref: &str) -> Result<()> {
    let shown = path.to_str().ok_or_else(|| anyhow!("worktree path is not valid UTF-8"))?;
    if path.exists() {
        git(repo, &["worktree", "remove", "--force", shown])?;
    }
    git(repo, &["worktree", "prune"])?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    git(repo, &["worktree", "add", "-B", branch, shown, base_ref])?;
    Ok(())
}

/// Files that differ between `base` and the work tree, committed or not.
pub fn changed_files(worktree: &Path, base: &str) -> Result<Vec<String>> {
    // Staging everything first makes new files show up in the diff.
    git(worktree, &["add", "-A"])?;
    let names = git(worktree, &["diff", "--cached", "--name-only", base])?;
    Ok(names.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
}

pub fn commit_all(worktree: &Path, subject: &str, body: &str) -> Result<String> {
    git(worktree, &["add", "-A"])?;
    git(worktree, &["commit", "-m", subject, "-m", body])?;
    head(worktree)
}

pub fn diff(worktree: &Path, base: &str, head: &str) -> Result<String> {
    git(worktree, &["diff", base, head])
}

pub fn remote_branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let found = git(repo, &["ls-remote", "--heads", "origin", branch])?;
    Ok(!found.trim().is_empty())
}
