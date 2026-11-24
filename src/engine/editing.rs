//! Changing a repository's workflow: checking a new `factory.yaml`, listing
//! the modules it may use, scaffolding a new module, and proposing a change.
//!
//! Nothing here changes the default branch. A run reads its workflow from
//! that branch so that it always uses reviewed files, and a proposal is a
//! pull request into it, for a person to merge.

use super::workflow::{self, ModuleDef, Workflow};
use super::{git, github, lock, repo_name, util};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

/// The default branch of a checkout, as `origin/<name>`.
fn base_of(repo_dir: &Path) -> Result<String> {
    git::git(repo_dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .context("could not find the default branch; run `git remote set-head origin --auto`")
}

fn read_base(repo_dir: &Path, base: &str, path: &str) -> Option<String> {
    git::git(repo_dir, &["show", &format!("{base}:{path}")]).ok()
}

/// The workflow file on the default branch.
pub fn current(repo_dir: &Path) -> Result<String> {
    let base = base_of(repo_dir)?;
    read_base(repo_dir, &base, "factory.yaml").ok_or_else(|| anyhow!("{base} has no factory.yaml"))
}

/// Checks a workflow file against the repository's modules and the built-in
/// ones. An error lists everything wrong with it, one line each.
pub fn check(repo_dir: &Path, text: &str) -> Result<std::result::Result<Workflow, Vec<String>>> {
    let base = base_of(repo_dir)?;
    let read_module = |module: &str, file: &str| read_base(repo_dir, &base, &format!("modules/{module}/{file}")).or_else(|| workflow::builtin(module, file));
    Ok(workflow::parse(text, &read_module).map_err(|e| {
        let message = e.to_string();
        let lines: Vec<String> = message.lines().filter_map(|l| l.trim().strip_prefix("- ")).map(str::to_string).collect();
        if lines.is_empty() {
            vec![message]
        } else {
            lines
        }
    }))
}

/// A workflow as a graph: the steps in order and where each failure goes.
pub fn graph(base: &str, parsed: &Workflow) -> Value {
    let step = |s: &workflow::Step| {
        json!({
            "name": s.name,
            "module": s.module,
            "kind": s.def.kind.name(),
            "vendor": s.vendor.map(|v| v.name()),
            "locks": s.def.locks,
        })
    };
    let mut steps: Vec<Value> = parsed.steps.iter().map(step).collect();
    // The engine's own steps follow the workflow's.
    let engine = |name: &str| json!({ "name": name, "module": format!("engine/{name}"), "kind": "engine", "vendor": null, "locks": [] });
    steps.extend([engine("sync"), engine("pr")]);
    let mut on_fail = parsed.on_fail.clone();
    if parsed.github.is_some() {
        steps.extend([engine("github"), engine("merge")]);
        if let Some(dev) = parsed.steps.iter().find(|s| s.def.role == Some(workflow::Role::Dev)) {
            on_fail.insert("github".to_string(), dev.name.clone());
        }
    }
    json!({
        "branch": base.trim_start_matches("origin/"),
        "steps": steps,
        "on_fail": on_fail,
        "judgment": parsed.judgment.as_ref().map(step),
        "summary": parsed.summary.as_ref().map(step),
        "limits": { "fix_rounds": parsed.fix_rounds, "github_fix_rounds": parsed.github_fix_rounds, "judgments": parsed.judgments },
    })
}

/// The graph of the workflow on the default branch, with the file's text.
pub fn current_graph(repo_dir: &Path) -> Result<Value> {
    let base = base_of(repo_dir)?;
    let text = current(repo_dir)?;
    let parsed = check(repo_dir, &text)?.map_err(|problems| anyhow!("factory.yaml is not valid:\n  - {}", problems.join("\n  - ")))?;
    let mut value = graph(&base, &parsed);
    value.as_object_mut().expect("a graph is an object").insert("text".into(), json!(text));
    Ok(value)
}

/// Every module a workflow in this repository may name: the built-in ones,
/// and the ones under `modules/` on the default branch. A repository module
/// with a built-in's name overrides it.
pub fn modules(repo_dir: &Path) -> Result<Vec<Value>> {
    let base = base_of(repo_dir)?;
    let listed = git::git(repo_dir, &["ls-tree", "-r", "--name-only", &base, "modules/"]).unwrap_or_default();
    let mut in_repo: Vec<String> = listed.lines().filter_map(|path| path.strip_prefix("modules/")?.strip_suffix("/module.yaml")).map(str::to_string).collect();
    in_repo.sort();
    let builtin: Vec<String> = workflow::builtin_modules().iter().map(|s| s.to_string()).collect();
    let mut names: Vec<String> = builtin.iter().chain(in_repo.iter()).cloned().collect();
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    for name in names {
        let text = read_base(repo_dir, &base, &format!("modules/{name}/module.yaml")).or_else(|| workflow::builtin(&name, "module.yaml")).unwrap_or_default();
        let source = match (builtin.contains(&name), in_repo.contains(&name)) {
            (true, true) => "repository, overriding the built-in",
            (false, true) => "repository",
            _ => "built-in",
        };
        match crate::yaml::from_str::<ModuleDef>(&text) {
            Ok(def) => out.push(json!({
                "module": name,
                "kind": def.kind.name(),
                "role": def.role.map(|r| format!("{r:?}").to_lowercase()),
                "needs": def.needs,
                "produces": def.produces,
                "locks": def.locks,
                "source": source,
                "problem": null,
            })),
            Err(problem) => out.push(json!({ "module": name, "kind": "?", "role": null, "needs": [], "produces": [], "locks": [], "source": source, "problem": problem })),
        }
    }
    Ok(out)
}

fn valid_part(part: &str) -> bool {
    !part.is_empty() && !part.starts_with('-') && !part.ends_with('-') && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The files of a new module, as paths from the repository root and their
/// text. `module` is `<kind>/<name>`. An agent module starts as a copy of the
/// built-in module for its role, and a built-in's own name starts an override.
pub fn scaffold(module: &str, role: Option<&str>) -> Result<Vec<(String, String)>> {
    let Some((kind, name)) = module.split_once('/') else {
        bail!("a module is named <kind>/<name>, such as check/lint, and '{module}' is not");
    };
    if !valid_part(name) {
        bail!("'{name}' is not a module name: use lowercase letters, digits and '-'");
    }
    let dir = format!("modules/{kind}/{name}");
    let copy_of = |source: &str| -> Vec<(String, String)> {
        workflow::builtin_files(source)
            .into_iter()
            .map(|(file, text)| {
                let text = if file == "module.yaml" {
                    // The module's own name is its first line.
                    text.replacen(&format!("name: {}", source.rsplit('/').next().unwrap_or("")), &format!("name: {name}"), 1)
                } else {
                    text.to_string()
                };
                (format!("{dir}/{file}"), text)
            })
            .collect()
    };
    if workflow::builtin_modules().contains(&module) {
        return Ok(copy_of(module));
    }
    match (kind, role) {
        ("check", None) => {
            let yaml = format!(
                "name: {name}\nkind: check\nneeds: [worktree, head_sha]\nproduces: [{}_log]\n# The command runs in the issue's worktree. Exit code 0 is the only pass.\nrun: ./{dir}/run.sh\npass_when: exit_code == 0\ntimeout_minutes: 10\n# Name what this check shares with other issues, such as the simulator.\nlocks: []\n",
                name.replace('-', "_")
            );
            let script = format!(
                "#!/bin/sh\n# The {name} check. Exit 0 means pass. Nothing here may call an LLM.\n#\n# It fails until someone writes it: a check that passes without checking\n# anything is worse than no check.\nset -eu\necho \"{name}: not written yet\" >&2\nexit 1\n"
            );
            Ok(vec![(format!("{dir}/module.yaml"), yaml), (format!("{dir}/run.sh"), script)])
        }
        ("check", Some(_)) => bail!("a check module has no role"),
        ("agent", Some(role @ ("research" | "acceptance" | "dev" | "review" | "summary"))) => Ok(copy_of(&format!("agent/{role}"))),
        ("agent", Some(other)) => bail!("'{other}' is not a role: use research, acceptance, dev, review or summary"),
        ("agent", None) => bail!("an agent module needs a role: research, acceptance, dev, review or summary. It starts as a copy of the built-in module for that role"),
        ("judgment", None) => Ok(copy_of("judgment/stuck")),
        ("judgment", Some(_)) => bail!("a judgment module has no role"),
        (other, _) => bail!("'{other}' is not a kind of module: use agent, check or judgment"),
    }
}

/// Puts files on a new branch cut from the default branch and opens a pull
/// request for them into it. Returns the pull request's URL.
fn propose(repo_dir: &Path, branch: &str, files: &[(String, String)], title: &str, body: &str) -> Result<String> {
    let repo_dir = repo_dir.canonicalize().with_context(|| format!("{} does not exist", repo_dir.display()))?;
    let repo = repo_name(&repo_dir)?;
    let home = util::home()?;
    let _git = lock::acquire(&home, &format!("git-{}", util::repo_dir_name(&repo)), &|| {})?;
    git::git(&repo_dir, &["fetch", "--quiet", "origin"])?;
    let base = base_of(&repo_dir)?;
    let dir = home.join("worktrees").join(util::repo_dir_name(&repo)).join(format!("proposal-{}", branch.replace('/', "-")));
    let shown = dir.display().to_string();
    let _ = git::git(&repo_dir, &["worktree", "remove", "--force", &shown]);
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    git::git(&repo_dir, &["worktree", "add", "--quiet", "-B", branch, &shown, &base])?;
    let result = (|| -> Result<String> {
        for (path, text) in files {
            let file = dir.join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&file, text).with_context(|| format!("writing {path}"))?;
            if path.ends_with(".sh") {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))?;
            }
        }
        if git::changed_files(&dir, &base)?.is_empty() {
            bail!("nothing would change: the default branch already has exactly this");
        }
        git::commit_all(&dir, title, body.lines().next().unwrap_or(""))?;
        git::git(&dir, &["push", "--quiet", "--force-with-lease", "-u", "origin", branch])?;
        let target = base.trim_start_matches("origin/");
        match github::find_pr(&repo, branch)? {
            Some((_, url)) => Ok(url),
            None => Ok(github::open_pr(&repo, target, branch, title, body)?.1),
        }
    })();
    let _ = git::git(&repo_dir, &["worktree", "remove", "--force", &shown]);
    let _ = git::git(&repo_dir, &["branch", "-D", branch]);
    result
}

/// Proposes a new workflow file. It is checked first: an invalid one is
/// never proposed, and the problems come back instead.
pub fn propose_workflow(repo_dir: &Path, text: &str) -> Result<std::result::Result<String, Vec<String>>> {
    if let Err(problems) = check(repo_dir, text)? {
        return Ok(Err(problems));
    }
    let branch = format!("factory/workflow-{}", util::run_id().to_lowercase());
    let body = "Changes the workflow `factory run` follows in this repository.\n\nThe file was checked before this pull request was opened: every module exists, every step's needs are produced earlier, and every failure goes back to an earlier step.\n\nA run reads its workflow from the default branch, so nothing changes until a person merges this.";
    propose(repo_dir, &branch, &[("factory.yaml".to_string(), text.to_string())], "chore: change the workflow", body).map(Ok)
}

/// Proposes a new module's files.
pub fn propose_module(repo_dir: &Path, module: &str, role: Option<&str>) -> Result<(String, Vec<String>)> {
    let files = scaffold(module, role)?;
    let existing = modules(repo_dir)?;
    if existing.iter().any(|m| m["module"] == module && m["source"].as_str().is_some_and(|s| s.starts_with("repository"))) {
        bail!("this repository already has modules/{module}");
    }
    let paths: Vec<String> = files.iter().map(|(path, _)| path.clone()).collect();
    let branch = format!("factory/module-{}", module.replace('/', "-"));
    let body = format!(
        "Adds `modules/{module}/`, scaffolded by `factory module new`.\n\nIt is a starting point: edit it in this pull request. A workflow can name `{module}` once this is merged.\n\nFiles:\n\n{}",
        paths.iter().map(|p| format!("- `{p}`")).collect::<Vec<_>>().join("\n")
    );
    let url = propose(repo_dir, &branch, &files, &format!("chore: add the {module} module"), &body)?;
    Ok((url, paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workflow that uses a scaffolded module in place of a built-in one,
    /// read through a function that serves the scaffold's files.
    fn parse_with(files: &[(String, String)], text: &str) -> std::result::Result<Workflow, String> {
        let read = |module: &str, file: &str| {
            files.iter().find(|(path, _)| *path == format!("modules/{module}/{file}")).map(|(_, text)| text.clone()).or_else(|| workflow::builtin(module, file))
        };
        workflow::parse(text, &read).map_err(|e| e.to_string())
    }

    const BASE: &str = "workflow:\n  - develop: { module: agent/dev, vendor: codex }\n  - verify:  { module: check/verify }\n  - review:  { module: agent/review, vendor: claude }\non_fail:\n  verify: develop\n  review: develop\n";

    #[test]
    fn a_scaffolded_check_is_a_valid_module_that_fails_until_written() {
        let files = scaffold("check/lint", None).unwrap();
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["modules/check/lint/module.yaml", "modules/check/lint/run.sh"]);
        assert!(files[1].1.contains("exit 1") && files[1].1.contains("not written yet"), "{}", files[1].1);
        // It can be named in a workflow straight away.
        let text = BASE.replace("  - review:", "  - lint:    { module: check/lint }\n  - review:").replace("on_fail:\n", "on_fail:\n  lint: develop\n");
        let workflow = parse_with(&files, &text).unwrap();
        assert_eq!(workflow.steps[2].def.run.as_deref(), Some("./modules/check/lint/run.sh"));
        assert_eq!(workflow.steps[2].def.produces, vec!["lint_log"]);
    }

    #[test]
    fn a_scaffolded_agent_starts_as_a_copy_of_its_roles_module() {
        let files = scaffold("agent/strict-review", Some("review")).unwrap();
        assert!(files.iter().any(|(p, t)| p == "modules/agent/strict-review/module.yaml" && t.starts_with("name: strict-review\n")));
        assert!(files.iter().any(|(p, _)| p == "modules/agent/strict-review/prompt.md"));
        let text = BASE.replace("module: agent/review", "module: agent/strict-review");
        assert_eq!(parse_with(&files, &text).unwrap().steps[2].module, "agent/strict-review");
        // A built-in's own name starts an override of it, with every file.
        let overridden = scaffold("agent/dev", None).unwrap();
        assert_eq!(overridden.len(), 4);
        assert!(overridden[0].1.starts_with("name: dev\n"));
        assert_eq!(scaffold("judgment/careful", None).unwrap().len(), 3);
    }

    #[test]
    fn a_scaffold_refuses_names_and_kinds_it_cannot_make() {
        let problem = |module: &str, role: Option<&str>| scaffold(module, role).unwrap_err().to_string();
        assert!(problem("lint", None).contains("named <kind>/<name>"));
        assert!(problem("check/Lint Me", None).contains("not a module name"));
        assert!(problem("check/../x", None).contains("not a module name"));
        assert!(problem("gate/scope", None).contains("not a kind of module"));
        assert!(problem("agent/helper", None).contains("needs a role"));
        assert!(problem("agent/helper", Some("boss")).contains("not a role"));
        assert!(problem("check/lint", Some("dev")).contains("has no role"));
    }
}
