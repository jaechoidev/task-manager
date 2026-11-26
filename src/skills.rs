//! The skills a project run by factory carries in `.claude/skills/`, for the
//! sessions people work in there: writing plans, reading what runs, and
//! promoting. They are kept in this repository's `skills/` and built into
//! the program, so a project's copy matches the engine that runs it.
//!
//! `factory repo init` writes them to the default branch, `factory skills
//! update` refreshes them, and the preflight compares them. Only folders
//! named `factory-*` belong to factory. Any other skill a project has is
//! left alone. The engine's own agents never see them: they run with
//! skills turned off.

use crate::gh;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

/// Where the skills factory owns live in a project.
pub const PREFIX: &str = ".claude/skills/factory-";

/// Each skill file as a project gets it.
pub const BUNDLED: &[(&str, &str)] = &[
    (".claude/skills/factory-plan/SKILL.md", include_str!("../skills/factory-plan/SKILL.md")),
    (".claude/skills/factory-promote/SKILL.md", include_str!("../skills/factory-promote/SKILL.md")),
    (".claude/skills/factory-status/SKILL.md", include_str!("../skills/factory-status/SKILL.md")),
];

/// What has to change on a branch for its factory skills to match.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drift {
    /// Files missing or different, to be written.
    pub write: Vec<String>,
    /// Files under a `factory-*` folder that factory no longer has.
    pub remove: Vec<String>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.write.is_empty() && self.remove.is_empty()
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.write.is_empty() {
            parts.push(format!("to write: {}", self.write.join(", ")));
        }
        if !self.remove.is_empty() {
            parts.push(format!("to remove: {}", self.remove.join(", ")));
        }
        parts.join("; ")
    }
}

/// Compares the factory skill files on a branch, each with its blob sha,
/// with the bundled ones.
pub fn drift(live: &[(String, String)], wanted: &[(String, String)]) -> Drift {
    let mut out = Drift::default();
    for (path, sha) in wanted {
        if !live.iter().any(|(p, s)| p == path && s == sha) {
            out.write.push(path.clone());
        }
    }
    for (path, _) in live {
        if path.starts_with(PREFIX) && !wanted.iter().any(|(p, _)| p == path) {
            out.remove.push(path.clone());
        }
    }
    out
}

/// The sha git gives a file with this text, as GitHub's trees list it.
fn blob_sha(text: &str) -> Result<String> {
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running git hash-object")?;
    child.stdin.take().ok_or_else(|| anyhow!("no stdin for git"))?.write_all(text.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("git hash-object failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn wanted() -> Result<Vec<(String, String)>> {
    BUNDLED.iter().map(|(path, text)| Ok((path.to_string(), blob_sha(text)?))).collect()
}

/// A branch as GitHub has it: its commit, that commit's tree, and the
/// factory skill files in it.
struct Branch {
    name: String,
    commit: String,
    tree: String,
    skills: Vec<(String, String)>,
}

fn branch(repo: &str) -> Result<Branch> {
    let info = gh::api("GET", &format!("repos/{repo}"), None)?;
    let name = info.get("default_branch").and_then(Value::as_str).ok_or_else(|| anyhow!("{repo} has no default branch"))?.to_string();
    let reference = gh::api("GET", &format!("repos/{repo}/git/ref/heads/{name}"), None)?;
    let commit = reference.pointer("/object/sha").and_then(Value::as_str).ok_or_else(|| anyhow!("no commit for {repo} {name}"))?.to_string();
    let tree = gh::api("GET", &format!("repos/{repo}/git/commits/{commit}"), None)?
        .pointer("/tree/sha")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("no tree for {repo} {commit}"))?
        .to_string();
    let listing = gh::api("GET", &format!("repos/{repo}/git/trees/{tree}?recursive=1"), None)?;
    if listing.get("truncated").and_then(Value::as_bool) == Some(true) {
        bail!("{repo}: GitHub listed only part of the tree of {name}, so its skills can't be compared");
    }
    let skills = listing
        .get("tree")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("blob"))
        .filter_map(|entry| Some((entry.get("path")?.as_str()?.to_string(), entry.get("sha")?.as_str()?.to_string())))
        .filter(|(path, _)| path.starts_with(PREFIX))
        .collect();
    Ok(Branch { name, commit, tree, skills })
}

/// What differs on the repository's default branch, and that branch's name.
pub fn status(repo: &str) -> Result<(String, Drift)> {
    let branch = branch(repo)?;
    Ok((branch.name.clone(), drift(&branch.skills, &wanted()?)))
}

/// Makes the default branch hold the bundled skills, in one commit made
/// through GitHub, so no checkout is needed. Returns the commit, or None
/// when they already match.
pub fn update(repo: &str) -> Result<Option<(String, Drift)>> {
    let branch = branch(repo)?;
    let change = drift(&branch.skills, &wanted()?);
    if change.is_empty() {
        return Ok(None);
    }
    let mut entries: Vec<Value> = Vec::new();
    for path in &change.write {
        let text = BUNDLED.iter().find(|(p, _)| p == path).map(|(_, text)| *text).unwrap_or_default();
        entries.push(json!({ "path": path, "mode": "100644", "type": "blob", "content": text }));
    }
    for path in &change.remove {
        entries.push(json!({ "path": path, "mode": "100644", "type": "blob", "sha": Value::Null }));
    }
    let tree = gh::api("POST", &format!("repos/{repo}/git/trees"), Some(&json!({ "base_tree": branch.tree, "tree": entries })))?;
    let tree = tree.get("sha").and_then(Value::as_str).ok_or_else(|| anyhow!("GitHub made no tree"))?;
    let message = format!("chore(skills): factory skills from factory {}", env!("CARGO_PKG_VERSION"));
    let commit = gh::api("POST", &format!("repos/{repo}/git/commits"), Some(&json!({ "message": message, "tree": tree, "parents": [branch.commit] })))?;
    let commit = commit.get("sha").and_then(Value::as_str).ok_or_else(|| anyhow!("GitHub made no commit"))?.to_string();
    // Not forced: if the branch moved meanwhile, this fails, and running it again is safe.
    gh::api("PATCH", &format!("repos/{repo}/git/refs/heads/{}", branch.name), Some(&json!({ "sha": commit, "force": false })))
        .with_context(|| format!("moving {} of {repo} to the new commit", branch.name))?;
    Ok(Some((commit, change)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(path: &str, sha: &str) -> (String, String) {
        (path.to_string(), sha.to_string())
    }

    #[test]
    fn only_what_differs_is_written_and_only_factorys_folders_are_cleared() {
        let wanted = vec![pair(".claude/skills/factory-plan/SKILL.md", "a1"), pair(".claude/skills/factory-status/SKILL.md", "b1")];
        assert!(drift(&wanted, &wanted).is_empty());
        let live = vec![
            pair(".claude/skills/factory-plan/SKILL.md", "a0"),
            pair(".claude/skills/factory-old/SKILL.md", "c0"),
            // Not factory's: never touched.
            pair(".claude/skills/release-notes/SKILL.md", "d0"),
        ];
        let change = drift(&live, &wanted);
        assert_eq!(change.write, vec![".claude/skills/factory-plan/SKILL.md", ".claude/skills/factory-status/SKILL.md"]);
        assert_eq!(change.remove, vec![".claude/skills/factory-old/SKILL.md"]);
        assert!(change.describe().contains("to remove: .claude/skills/factory-old/SKILL.md"));
    }

    #[test]
    fn every_bundled_skill_names_itself_after_its_folder() {
        for (path, text) in BUNDLED {
            let folder = path.trim_start_matches(".claude/skills/").split('/').next().unwrap();
            assert!(path.starts_with(PREFIX) && path.ends_with("/SKILL.md"), "{path}");
            assert!(text.starts_with(&format!("---\nname: {folder}\ndescription: ")), "{path} must start with its front matter");
        }
    }

    #[test]
    fn a_blob_sha_is_the_one_git_gives() {
        // `printf 'hello\n' | git hash-object --stdin`
        assert_eq!(blob_sha("hello\n").unwrap(), "ce013625030ba8dba906f756967f9e9ca394464a");
    }
}
