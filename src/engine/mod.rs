//! The issue loop, run by code. An agent saying "done" is never a pass.
//!
//! This version takes one issue through develop, verify and review, then
//! opens a pull request. There is no fix loop yet: a failed step stops the
//! run with its state recorded.

pub mod agent;
pub mod git;
pub mod issue;
pub mod proc;
pub mod state;
pub mod util;
pub mod workflow;

use crate::gh;
use anyhow::{anyhow, bail, Context, Result};
use issue::Issue;
use serde::Deserialize;
use serde_json::{json, Value};
use state::{StepRecord, Store};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use workflow::{Access, Kind, Role, Step, Workflow};

/// Longest diff handed to a reviewer inline. The reviewer can read the rest.
const MAX_DIFF_CHARS: usize = 150_000;

pub struct RunOptions {
    /// A checkout of the project repository.
    pub repo_dir: PathBuf,
    pub issue: u64,
    /// Use this workflow file, not the one on the base branch.
    pub workflow_file: Option<PathBuf>,
    /// Stop after review, without pushing or opening a pull request.
    pub no_pr: bool,
}

/// The developer's final message.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevReport {
    summary: String,
    commit_subject: String,
    files_changed: Vec<String>,
    verify_ran: String,
    notes: String,
    flow_change_request: Option<String>,
}

/// The reviewer's final message.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Verdict {
    verdict: String,
    head_sha: String,
    summary: String,
    items: Vec<String>,
}

enum StepResult {
    Pass,
    Fail(String),
}

struct Run<'a> {
    store: &'a Store,
    repo: String,
    issue: &'a Issue,
    run_id: String,
    run_dir: PathBuf,
    worktree: PathBuf,
    base_sha: String,
    head_sha: Option<String>,
    dev: Option<DevReport>,
    dev_vendor: Option<String>,
    verify: Option<String>,
    review: Option<(String, Verdict)>,
}

fn bullets(items: &[String], code: bool) -> String {
    if items.is_empty() {
        return "None.".to_string();
    }
    items
        .iter()
        .map(|item| if code { format!("- `{}`", item.trim()) } else { format!("- {}", item.trim()) })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    // Filled in order, so text inserted by one value is not scanned again for
    // the keys before it. Untrusted text such as the diff goes last.
    for (key, value) in values {
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

/// Changed files that lie outside every scope path.
pub fn outside_scope(files: &[String], scope: &[String]) -> Vec<String> {
    let roots: Vec<String> =
        scope.iter().map(|s| s.trim().trim_start_matches("./").trim_end_matches('/').to_string()).collect();
    files
        .iter()
        .filter(|file| !roots.iter().any(|root| *file == root || file.starts_with(&format!("{root}/"))))
        .cloned()
        .collect()
}

/// One line, at most 72 characters. Falls back when the agent's is unusable.
fn commit_subject(proposed: &str, issue: &Issue) -> String {
    let line = proposed.lines().next().unwrap_or("").trim();
    if line.is_empty() || line.chars().count() > 72 {
        let mut fallback = format!("feat: {}", issue.title.trim());
        if fallback.chars().count() > 72 {
            fallback = fallback.chars().take(72).collect();
        }
        return fallback;
    }
    line.to_string()
}

fn repo_name(repo_dir: &Path) -> Result<String> {
    let output = Command::new("gh")
        .args(["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
        .current_dir(repo_dir)
        .output()
        .context("running gh")?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || name.is_empty() {
        bail!("{} is not a checkout of a GitHub repository", repo_dir.display());
    }
    Ok(name)
}

/// Labels mirror the state for people. A failure here never fails the run.
fn label(repo: &str, number: u64, add: Option<&str>, remove: Option<&str>) {
    if let Some(name) = remove {
        let _ = gh::api("DELETE", &format!("repos/{repo}/issues/{number}/labels/{}", gh::encode(name)), None);
    }
    if let Some(name) = add {
        if let Err(e) = gh::api("POST", &format!("repos/{repo}/issues/{number}/labels"), Some(&json!({ "labels": [name] }))) {
            eprintln!("warning: could not add label {name}: {e}");
        }
    }
}

fn seconds(duration: Duration) -> String {
    format!("{:.0} s", duration.as_secs_f64())
}

impl Run<'_> {
    fn record(&self, step: &Step, attempt: u32, verdict: &str, outcome: Option<&proc::Outcome>, files: [Option<&Path>; 4], session: Option<String>) -> Result<()> {
        let path = |p: Option<&Path>| p.map(|p| p.display().to_string());
        self.store.record(&StepRecord {
            ts: util::now(),
            repo: self.repo.clone(),
            issue: self.issue.number,
            run_id: self.run_id.clone(),
            step: step.name.clone(),
            kind: step.def.kind.name().to_string(),
            module: step.module.clone(),
            vendor: step.vendor.map(|v| v.name().to_string()),
            model: step.model.clone(),
            subscription: step.vendor.map(|_| "default".to_string()),
            attempt,
            sha: self.head_sha.clone(),
            exit_code: outcome.and_then(|o| o.exit_code),
            timed_out: outcome.is_some_and(|o| o.timed_out),
            verdict: verdict.to_string(),
            duration_ms: outcome.map_or(0, |o| o.duration.as_millis() as u64),
            input: path(files[0]),
            output: path(files[1]),
            stdout: path(files[2]),
            stderr: path(files[3]),
            session_id: session,
        })
    }

    /// Logs one of the engine's fixed gates. `failure` is why it failed.
    fn gate(&self, name: &str, failure: Option<String>) -> Result<StepResult> {
        self.store.record(&StepRecord {
            ts: util::now(),
            repo: self.repo.clone(),
            issue: self.issue.number,
            run_id: self.run_id.clone(),
            step: name.to_string(),
            kind: "gate".to_string(),
            module: format!("engine/{name}"),
            vendor: None,
            model: None,
            subscription: None,
            attempt: 1,
            sha: self.head_sha.clone(),
            exit_code: None,
            timed_out: false,
            verdict: if failure.is_some() { "fail" } else { "pass" }.to_string(),
            duration_ms: 0,
            input: None,
            output: None,
            stdout: None,
            stderr: None,
            session_id: None,
        })?;
        Ok(match failure {
            Some(reason) => StepResult::Fail(format!("{name} gate: {reason}")),
            None => StepResult::Pass,
        })
    }

    /// Calls an agent and parses its reply. An unusable reply gets one retry.
    /// `verdict_of` names what goes in the log for a usable reply.
    fn ask<T: for<'de> Deserialize<'de>>(&self, index: usize, step: &Step, prompt: &str, verdict_of: fn(&T) -> String) -> Result<Result<(T, Value), String>> {
        let vendor = step.vendor.ok_or_else(|| anyhow!("step '{}' has no vendor", step.name))?;
        let schema = step.schema.as_deref().unwrap_or("{}");
        let access = step.def.access.unwrap_or(Access::ReadOnly);
        let timeout = Duration::from_secs(step.def.timeout_minutes.unwrap_or(30) * 60);
        let mut prompt = prompt.to_string();
        let mut last_error = String::new();

        for attempt in 1..=2u32 {
            println!("  {}: {} call, attempt {attempt}", step.name, vendor.name());
            let stem = format!("{:02}-{}-{attempt}", index + 1, step.name);
            let reply = agent::call(&agent::Call {
                vendor,
                model: step.model.as_deref(),
                access,
                cwd: &self.worktree,
                prompt: &prompt,
                schema,
                dir: &self.run_dir,
                stem: &stem,
                timeout,
            })?;
            let parsed = match (&reply.output, &reply.error) {
                (Some(value), None) => serde_json::from_value::<T>(value.clone())
                    .map(|typed| (typed, value.clone()))
                    .map_err(|e| format!("reply does not match the schema: {e}")),
                (_, Some(error)) => Err(error.clone()),
                (None, None) => Err("no reply".to_string()),
            };
            let files = [
                Some(reply.input_path.as_path()),
                Some(reply.output_path.as_path()),
                Some(reply.stdout_path.as_path()),
                Some(reply.stderr_path.as_path()),
            ];
            match parsed {
                Ok(found) => {
                    println!("  {}: replied in {}", step.name, seconds(reply.outcome.duration));
                    self.record(step, attempt, &verdict_of(&found.0), Some(&reply.outcome), files, reply.session_id)?;
                    return Ok(Ok(found));
                }
                Err(error) => {
                    println!("  {}: unusable reply ({error})", step.name);
                    self.record(step, attempt, "invalid", Some(&reply.outcome), files, reply.session_id)?;
                    // A timeout or a crash is not a format problem. Don't repeat it.
                    if reply.outcome.timed_out {
                        return Ok(Err(error));
                    }
                    prompt = format!("{prompt}\n\n# Your previous reply was rejected\n\n{error}\n\nReply again with only the JSON object.");
                    last_error = error;
                }
            }
        }
        Ok(Err(format!("two unusable replies in a row. Last problem: {last_error}")))
    }

    fn issue_values(&self) -> Vec<(&'static str, String)> {
        let marker = &self.issue.marker;
        let flows: Vec<String> =
            marker.flows.iter().map(|f| format!("{}{}", f.file, if f.new { " (new)" } else { "" })).collect();
        vec![
            ("number", self.issue.number.to_string()),
            ("title", self.issue.title.clone()),
            ("summary", self.issue.summary.clone()),
            ("scope", bullets(&marker.scope, true)),
            ("accept", bullets(&marker.accept, false)),
            ("accessibility_ids", bullets(&marker.accessibility_ids, true)),
            ("flows", bullets(&flows, true)),
        ]
    }

    fn develop(&mut self, index: usize, step: &Step) -> Result<StepResult> {
        let values = self.issue_values();
        let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let prompt = render(step.prompt.as_deref().unwrap_or(""), &pairs);

        let (report, _) = match self.ask::<DevReport>(index, step, &prompt, |_| "replied".to_string())? {
            Ok(found) => found,
            Err(error) => return Ok(StepResult::Fail(error)),
        };

        // Fixed gates. None of these can be turned off by a workflow file.
        if git::head(&self.worktree)? != self.base_sha {
            return self.gate("history", Some("the developer changed git history; only the engine commits".into()));
        }
        let changed = git::changed_files(&self.worktree, &self.base_sha)?;
        if changed.is_empty() {
            return self.gate("changes", Some(format!("the developer changed no files. Its notes: {}", report.notes)));
        }
        let flows: Vec<&String> = changed.iter().filter(|f| f.starts_with(".maestro/")).collect();
        if !flows.is_empty() {
            return self.gate("locked-flows", Some(format!("acceptance flows are locked, but the change touches {flows:?}")));
        }
        self.gate("locked-flows", None)?;
        let outside = outside_scope(&changed, &self.issue.marker.scope);
        if !outside.is_empty() {
            return self.gate("scope", Some(format!("files outside the issue's scope were changed: {}", outside.join(", "))));
        }
        self.gate("scope", None)?;
        println!("  scope gate: {} file(s), all inside scope", changed.len());

        let subject = commit_subject(&report.commit_subject, self.issue);
        let body = format!("Issue #{}", self.issue.number);
        let head = git::commit_all(&self.worktree, &subject, &body)?;
        println!("  committed {} \"{subject}\"", &head[..head.len().min(10)]);
        self.store.set_head(&self.repo, self.issue.number, &head)?;
        self.head_sha = Some(head);
        self.dev_vendor = step.vendor.map(|v| v.name().to_string());
        self.dev = Some(report);
        Ok(StepResult::Pass)
    }

    /// Every later step must see exactly the commit the developer produced.
    fn same_commit(&self, who: &str) -> Result<Option<String>> {
        let expected = self.head_sha.clone().ok_or_else(|| anyhow!("no commit to check yet"))?;
        if git::head(&self.worktree)? != expected {
            return Ok(Some(format!("{who} moved the branch away from {expected}")));
        }
        let dirty = git::dirty(&self.worktree)?;
        if !dirty.is_empty() {
            return Ok(Some(format!("{who} left the worktree changed:\n{dirty}")));
        }
        Ok(None)
    }

    fn check(&mut self, index: usize, step: &Step) -> Result<StepResult> {
        let command = step.def.run.clone().unwrap_or_default();
        let stem = format!("{:02}-{}", index + 1, step.name);
        let stdout = self.run_dir.join(format!("{stem}.stdout.log"));
        let stderr = self.run_dir.join(format!("{stem}.stderr.log"));
        println!("  {}: running {command}", step.name);
        let outcome = proc::run(&proc::Spec {
            program: "/bin/sh",
            args: vec!["-c".into(), command.clone()],
            cwd: &self.worktree,
            stdin: None,
            stdout: &stdout,
            stderr: &stderr,
            timeout: Duration::from_secs(step.def.timeout_minutes.unwrap_or(20) * 60),
        })?;
        let passed = outcome.ok();
        self.record(step, 1, if passed { "pass" } else { "fail" }, Some(&outcome), [None, None, Some(stdout.as_path()), Some(stderr.as_path())], None)?;

        if !passed {
            let why = if outcome.timed_out { "timed out".to_string() } else { format!("exit code {:?}", outcome.exit_code) };
            self.verify = Some(format!("failed ({why})"));
            return Ok(StepResult::Fail(format!("{command} failed: {why}. Log: {}", stderr.display())));
        }
        if let Some(problem) = self.same_commit(&command)? {
            return Ok(StepResult::Fail(problem));
        }
        self.verify = Some(format!("passed (exit 0, {})", seconds(outcome.duration)));
        println!("  {}: passed in {}", step.name, seconds(outcome.duration));
        Ok(StepResult::Pass)
    }

    fn review(&mut self, index: usize, step: &Step) -> Result<StepResult> {
        let head = self.head_sha.clone().ok_or_else(|| anyhow!("review needs a commit"))?;
        let mut diff = git::diff(&self.worktree, &self.base_sha, &head)?;
        if diff.len() > MAX_DIFF_CHARS {
            let cut = (0..=MAX_DIFF_CHARS).rev().find(|&i| diff.is_char_boundary(i)).unwrap_or(0);
            diff.truncate(cut);
            diff.push_str("\n[diff cut here: read the files for the rest]");
        }
        let dev = self.dev.as_ref().ok_or_else(|| anyhow!("review needs the developer's report"))?;
        let dev_report = format!(
            "{}\n\nFiles it lists: {}\n\nIts own verify run: {}\n\nNotes: {}",
            dev.summary,
            dev.files_changed.join(", "),
            dev.verify_ran,
            if dev.notes.trim().is_empty() { "none" } else { dev.notes.trim() }
        );
        let verify = self.verify.clone().unwrap_or_else(|| "not run".to_string());
        let mut values = self.issue_values();
        values.extend([
            ("head_sha", head.clone()),
            ("base_sha", self.base_sha.clone()),
            ("verify_result", verify),
            ("dev_report", dev_report),
            ("diff", diff),
        ]);
        let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let prompt = render(step.prompt.as_deref().unwrap_or(""), &pairs);

        let (verdict, _) = match self.ask::<Verdict>(index, step, &prompt, |v| v.verdict.clone())? {
            Ok(found) => found,
            Err(error) => return Ok(StepResult::Fail(error)),
        };
        if let Some(problem) = self.same_commit("the reviewer")? {
            return Ok(StepResult::Fail(problem));
        }
        // One SHA for every signal: a verdict on another commit is worthless.
        let reviewed = verdict.head_sha.trim();
        if reviewed.len() < 7 || !head.starts_with(reviewed) {
            return Ok(StepResult::Fail(format!("the reviewer judged commit '{reviewed}', not {head}")));
        }
        let vendor = step.vendor.map(|v| v.name()).unwrap_or("?").to_string();
        println!("  {}: {} by {vendor}", step.name, verdict.verdict);
        let result = match verdict.verdict.as_str() {
            "approve" => StepResult::Pass,
            "changes" => StepResult::Fail(format!(
                "review asked for changes:\n{}\n{}",
                verdict.summary,
                bullets(&verdict.items, false)
            )),
            other => StepResult::Fail(format!("the reviewer returned an unknown verdict '{other}'")),
        };
        self.review = Some((vendor, verdict));
        Ok(result)
    }

    fn pr_body(&self) -> String {
        let head = self.head_sha.as_deref().unwrap_or("?");
        let mut out = format!("Closes #{}\n\n## What changed\n\n", self.issue.number);
        if let Some(dev) = &self.dev {
            out.push_str(dev.summary.trim());
            out.push_str("\n\n");
        }
        out.push_str("## Verification\n\n");
        out.push_str(&format!("- Commit verified: `{head}`\n"));
        out.push_str("- Scope gate: passed, every changed file is inside the issue's scope\n");
        out.push_str(&format!("- `./verify.sh`: {}\n", self.verify.as_deref().unwrap_or("not run")));
        if let Some((vendor, verdict)) = &self.review {
            out.push_str(&format!("- Review ({vendor}): {} on `{}`\n", verdict.verdict, verdict.head_sha.trim()));
        }
        out.push_str("\n## For a human to check\n\n");
        let mut notes: Vec<String> = Vec::new();
        if let Some(dev) = &self.dev {
            if !dev.notes.trim().is_empty() {
                notes.push(format!("Developer: {}", dev.notes.trim()));
            }
            if let Some(request) = dev.flow_change_request.as_deref().filter(|r| !r.trim().is_empty()) {
                notes.push(format!("Flow change requested: {}", request.trim()));
            }
        }
        if let Some((_, verdict)) = &self.review {
            notes.push(format!("Reviewer: {}", verdict.summary.trim()));
            notes.extend(verdict.items.iter().map(|i| i.trim().to_string()));
        }
        out.push_str(&bullets(&notes, false));
        out.push_str(&format!(
            "\n\n---\nOpened by `factory run`, run `{}`. Developer: {}. Every verdict above is for the commit named.\n",
            self.run_id,
            self.dev_vendor.as_deref().unwrap_or("?")
        ));
        out
    }
}

pub fn run(opts: &RunOptions) -> Result<bool> {
    let repo_dir = opts.repo_dir.canonicalize().with_context(|| format!("{} does not exist", opts.repo_dir.display()))?;
    let repo = repo_name(&repo_dir)?;
    let home = util::home()?;
    let store = Store::open(&home)?;

    let issue = issue::load(&repo, opts.issue)?;
    println!("run {repo}#{} {}/{}: {}", issue.number, issue.plan, issue.key, issue.title);

    git::git(&repo_dir, &["fetch", "--quiet", "origin"])?;
    let base = git::git(&repo_dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .context("could not find the default branch; run `git remote set-head origin --auto`")?
        .trim_start_matches("origin/")
        .to_string();
    let base_ref = format!("origin/{base}");
    let base_sha = git::git(&repo_dir, &["rev-parse", &base_ref])?;

    // The workflow and any repo modules are read from the base branch, never
    // from a working copy, so a run always uses reviewed files.
    let read_base = |path: &str| git::git(&repo_dir, &["show", &format!("{base_ref}:{path}")]).ok();
    let workflow_text = match &opts.workflow_file {
        Some(file) => std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?,
        None => read_base("factory.yaml").ok_or_else(|| anyhow!("{base_ref} has no factory.yaml"))?,
    };
    let read_module = |module: &str, file: &str| {
        read_base(&format!("modules/{module}/{file}")).or_else(|| workflow::builtin(module, file))
    };
    let Workflow { steps, .. } = workflow::parse(&workflow_text, &read_module)?;

    let branch = format!("factory/issue-{}-{}", issue.number, issue.key);
    if git::remote_branch_exists(&repo_dir, &branch)? {
        bail!("branch {branch} is already on GitHub from an earlier run. Close its pull request and delete the branch before running this issue again");
    }

    let run_id = util::run_id();
    let name = util::repo_dir_name(&repo);
    let worktree = home.join("worktrees").join(&name).join(format!("issue-{}", issue.number));
    let run_dir = home.join("runs").join(&name).join(format!("issue-{}", issue.number)).join(&run_id);
    std::fs::create_dir_all(&run_dir)?;
    git::fresh_worktree(&repo_dir, &worktree, &branch, &base_ref)?;
    println!("  worktree {} on {branch}, from {base} at {}", worktree.display(), &base_sha[..10]);
    println!("  files for this run: {}", run_dir.display());

    store.start_issue(&repo, issue.number, &issue.plan, &issue.key, &issue.title, &run_id, &branch, &worktree.display().to_string(), &base_sha)?;
    label(&repo, issue.number, Some("in-progress"), Some("blocked"));

    let mut run = Run {
        store: &store,
        repo: repo.clone(),
        issue: &issue,
        run_id,
        run_dir,
        worktree: worktree.clone(),
        base_sha,
        head_sha: None,
        dev: None,
        dev_vendor: None,
        verify: None,
        review: None,
    };

    for (index, step) in steps.iter().enumerate() {
        store.set_stage(&repo, issue.number, &step.name, "running", None)?;
        let result = match (step.def.kind, step.def.role) {
            (Kind::Agent, Some(Role::Dev)) => run.develop(index, step),
            (Kind::Agent, Some(Role::Review)) => run.review(index, step),
            (Kind::Check, _) => run.check(index, step),
            _ => Err(anyhow!("step '{}' has a kind the engine can't run yet", step.name)),
        };
        let failure = match result {
            Ok(StepResult::Pass) => continue,
            Ok(StepResult::Fail(reason)) => reason,
            Err(e) => format!("engine error: {e:#}"),
        };
        store.set_stage(&repo, issue.number, &step.name, "failed", Some(&failure))?;
        label(&repo, issue.number, Some("blocked"), Some("in-progress"));
        println!("FAILED at {}: {failure}", step.name);
        println!("  no fix loop yet, so the run stops here. Worktree kept at {}", worktree.display());
        return Ok(false);
    }

    if opts.no_pr {
        store.set_stage(&repo, issue.number, "reviewed", "passed", None)?;
        println!("PASSED every step. No pull request opened (--no-pr)");
        return Ok(true);
    }

    store.set_stage(&repo, issue.number, "pr", "running", None)?;
    git::git(&worktree, &["push", "--quiet", "-u", "origin", &branch])?;
    let body_file = run.run_dir.join("pr-body.md");
    std::fs::write(&body_file, run.pr_body())?;
    let title = git::git(&worktree, &["log", "-1", "--format=%s"])?;
    let output = Command::new("gh")
        .args(["pr", "create", "--base", &base, "--head", &branch, "--title", &title, "--body-file"])
        .arg(&body_file)
        .current_dir(&worktree)
        .output()
        .context("running gh pr create")?;
    if !output.status.success() {
        let failure = format!("gh pr create failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        store.set_stage(&repo, issue.number, "pr", "failed", Some(&failure))?;
        println!("FAILED at pr: {failure}");
        return Ok(false);
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().lines().last().unwrap_or("").to_string();
    let number = url.rsplit('/').next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
    store.set_pr(&repo, issue.number, number, &url)?;
    store.set_stage(&repo, issue.number, "pr", "open", None)?;
    println!("PASSED every step. Pull request: {url}");
    Ok(true)
}

pub fn status() -> Result<()> {
    let home = util::home()?;
    let store = Store::open(&home)?;
    let issues = store.issues()?;
    if issues.is_empty() {
        println!("no issues have been run yet ({})", home.display());
        return Ok(());
    }
    for row in issues {
        println!("{}#{} {}  {}", row.repo, row.number, row.key, row.title);
        println!("  stage {} ({}), run {}, updated {}", row.stage, row.status, row.run_id, row.updated_at);
        if let Some(sha) = &row.head_sha {
            println!("  commit {sha}");
        }
        if let Some(url) = &row.pr_url {
            println!("  pull request {url}");
        }
        for (step, vendor, attempt, verdict, exit_code, duration_ms, _) in store.steps(&row.repo, row.number, &row.run_id)? {
            let who = vendor.map(|v| format!(" [{v}]")).unwrap_or_default();
            let code = exit_code.map(|c| c.to_string()).unwrap_or_else(|| "-".into());
            println!("    {step}{who} attempt {attempt}: {verdict}, exit {code}, {:.1} s", duration_ms as f64 / 1000.0);
        }
        if let Some(failure) = &row.failure {
            println!("  failure: {}", failure.lines().next().unwrap_or(""));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_gate_matches_whole_path_parts() {
        let scope = vec!["Sources/App/".to_string(), "./Sources/UI/List".to_string(), "README.md".to_string()];
        let files: Vec<String> = [
            "Sources/App/TodoApp.swift",
            "Sources/UI/List/Row/RowView.swift",
            "README.md",
            "Sources/UI/ListDetail/Detail.swift",
            "Sources/Application.swift",
            ".maestro/add.yaml",
            "TodoTrial.xcodeproj/project.pbxproj",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            outside_scope(&files, &scope),
            vec![
                "Sources/UI/ListDetail/Detail.swift",
                "Sources/Application.swift",
                ".maestro/add.yaml",
                "TodoTrial.xcodeproj/project.pbxproj"
            ]
        );
    }

    #[test]
    fn templates_are_filled() {
        let out = render("#{{number}} {{title}} / {{number}}", &[("number", "4"), ("title", "Shell")]);
        assert_eq!(out, "#4 Shell / 4");
    }
}
