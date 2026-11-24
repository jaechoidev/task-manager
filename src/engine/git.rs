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

/// Whether a tracked or staged file under `paths` contains `needle`. With no
/// paths, every file counts except flows, plans and prose, which name IDs
/// without implementing them.
pub fn in_code(worktree: &Path, needle: &str, paths: &[String]) -> Result<bool> {
    let mut args: Vec<String> = ["grep", "-q", "-F", "-e", needle, "--"].iter().map(|s| s.to_string()).collect();
    if paths.is_empty() {
        args.extend([".", ":(exclude).maestro", ":(exclude)plans", ":(exclude)*.md"].iter().map(|s| s.to_string()));
    } else {
        args.extend(paths.iter().map(|p| p.trim().trim_start_matches("./").trim_end_matches('/').to_string()));
    }
    let status = Command::new("git")
        .args(&args)
        .current_dir(worktree)
        .status()
        .context("running git grep")?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        other => bail!("git grep failed with {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_found_in_code_but_not_in_flows_plans_or_prose() {
        let dir = std::env::temp_dir().join(format!("factory-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["Sources/UI/Add", "Sources/UI/List", ".maestro", "plans"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("Sources/UI/Add/AddView.swift"), "Button(\"Save\").accessibilityIdentifier(\"todo.add.save\")").unwrap();
        std::fs::write(dir.join("Sources/UI/List/ListView.swift"), "List {}.accessibilityIdentifier(\"todo.list\")").unwrap();
        std::fs::write(dir.join(".maestro/add.yaml"), "- tapOn:\n    id: \"todo.add.field\"\n").unwrap();
        std::fs::write(dir.join("plans/p.yaml"), "accessibility_ids: [\"todo.add.field\"]\n").unwrap();
        std::fs::write(dir.join("NOTES.md"), "Uses \"todo.add.field\".\n").unwrap();
        git(&dir, &["init", "--quiet"]).unwrap();
        git(&dir, &["add", "-A"]).unwrap();

        let anywhere: &[String] = &[];
        assert!(in_code(&dir, "\"todo.add.save\"", anywhere).unwrap());
        assert!(in_code(&dir, "\"todo.list\"", anywhere).unwrap());
        // Named in a flow, a plan and a note, but implemented nowhere.
        assert!(!in_code(&dir, "\"todo.add.field\"", anywhere).unwrap());
        // A longer ID does not satisfy a shorter one, and scope narrows the search.
        assert!(!in_code(&dir, "\"todo.add\"", anywhere).unwrap());
        let scope = vec!["./Sources/UI/Add/".to_string()];
        assert!(in_code(&dir, "\"todo.add.save\"", &scope).unwrap());
        assert!(!in_code(&dir, "\"todo.list\"", &scope).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
