//! The issue loop, run by code. An agent saying "done" is never a pass.
//!
//! This version takes one issue through acceptance flows, develop, verify and
//! review, opens a pull request, waits for GitHub's checks and review on the
//! same commit, and merges. A failure the developer can fix sends the issue
//! back to it, up to a round limit.

pub mod agent;
pub mod flows;
pub mod git;
pub mod github;
pub mod issue;
pub mod proc;
pub mod rounds;
pub mod state;
pub mod util;
pub mod workflow;

use crate::gh;
use anyhow::{anyhow, bail, Context, Result};
use issue::Issue;
use rounds::{End, Feedback, Limits, Outcome};
use serde::Deserialize;
use serde_json::{json, Value};
use state::{StepRecord, Store};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use workflow::{Access, Kind, Role, Step, Workflow};

/// Longest diff handed to a reviewer inline. The reviewer can read the rest.
const MAX_DIFF_CHARS: usize = 150_000;
/// How much of a failed check's output the developer is shown.
const LOG_TAIL_LINES: usize = 120;
const LOG_TAIL_CHARS: usize = 12_000;

pub struct RunOptions {
    /// A checkout of the project repository.
    pub repo_dir: PathBuf,
    pub issue: u64,
    /// Use this workflow file, not the one on the base branch.
    pub workflow_file: Option<PathBuf>,
    /// Stop after review, without pushing or opening a pull request.
    pub no_pr: bool,
    /// Follow the pull request on GitHub, but don't merge it.
    pub no_merge: bool,
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

/// The acceptance writer's final message.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceReport {
    summary: String,
    /// Kept for the log. The engine reads what was written from git.
    #[allow(dead_code)]
    flows: Vec<String>,
    added_ids: Vec<String>,
    notes: String,
}

/// The acceptance writer's answer to a flow change request.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ruling {
    decision: String,
    reason: String,
}

/// What the acceptance stage left behind.
struct Acceptance {
    vendor: String,
    sha: String,
    report: AcceptanceReport,
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

/// What a step left on disk, for the request log.
#[derive(Default)]
struct Evidence<'a> {
    outcome: Option<&'a proc::Outcome>,
    /// Input, output, stdout, stderr.
    files: [Option<&'a Path>; 4],
    session: Option<String>,
}

/// One agent call: what to send and how to name it.
struct Ask<'a> {
    /// The step's name in the log and in file names.
    name: &'a str,
    prompt: &'a str,
    schema: Option<&'a str>,
    /// A session to continue.
    resume: Option<&'a str>,
}

/// A fix round that ended in a commit.
struct Fix {
    round: u32,
    /// The step whose failure caused the round.
    from: String,
    /// The first line of what failed.
    why: String,
    commit: String,
    subject: String,
}

struct Run<'a> {
    store: &'a Store,
    repo: String,
    issue: &'a Issue,
    run_id: String,
    run_dir: PathBuf,
    worktree: PathBuf,
    /// The checkout the worktree belongs to.
    repo_dir: PathBuf,
    /// The branch pull requests go into, and its remote-tracking name.
    base: String,
    base_ref: String,
    branch: String,
    base_sha: String,
    /// Where the branch must be right now: the engine's latest commit.
    expected: String,
    /// The commit verify and review judge. Set once development is committed.
    head_sha: Option<String>,
    acceptance: Option<Acceptance>,
    /// The result of the check that new flows fail before development.
    red: Option<String>,
    /// Every accessibility ID development must implement: the plan's, plus
    /// any the acceptance writer added.
    ids: Vec<String>,
    /// 0 for the first pass, then one more each time the run goes back.
    round: u32,
    fixes: Vec<Fix>,
    /// The pull request, once open: number and URL.
    pr: Option<(u64, String)>,
    github: Option<github::Settings>,
    /// When the branch was last pushed.
    pushed: Option<Instant>,
    merged: bool,
    dev: Option<DevReport>,
    dev_vendor: Option<String>,
    dev_subject: Option<String>,
    /// The subject the developer proposed on its first attempt. It names the
    /// whole change even when a gate held that attempt back and the first
    /// commit comes out of a fix round.
    first_subject: Option<String>,
    /// The developer's session, continued in fix rounds.
    dev_session: Option<String>,
    /// The review before the current one, when it asked for changes.
    earlier_review: Option<Verdict>,
    /// An accepted or rejected flow change request, with the commit of an
    /// accepted change.
    ruling: Option<(Ruling, Option<String>)>,
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

/// The end of a log file, for a prompt.
fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n");
    if out.len() > LOG_TAIL_CHARS {
        let cut = (out.len() - LOG_TAIL_CHARS..out.len()).find(|&i| out.is_char_boundary(i)).unwrap_or(0);
        out = out[cut..].to_string();
    }
    out.trim().to_string()
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let mut out: String = line.chars().take(160).collect();
    if line.chars().count() > 160 {
        out.push_str("...");
    }
    out
}

fn seconds(duration: Duration) -> String {
    format!("{:.0} s", duration.as_secs_f64())
}

impl Run<'_> {
    /// `name` is the step's name in the log. It differs from `step.name` when
    /// one module does a second job, such as ruling on a flow change request.
    fn record(&self, step: &Step, name: &str, attempt: u32, verdict: &str, evidence: Evidence) -> Result<()> {
        let Evidence { outcome, files, session } = evidence;
        let path = |p: Option<&Path>| p.map(|p| p.display().to_string());
        self.store.record(&StepRecord {
            ts: util::now(),
            repo: self.repo.clone(),
            issue: self.issue.number,
            run_id: self.run_id.clone(),
            step: name.to_string(),
            kind: step.def.kind.name().to_string(),
            module: step.module.clone(),
            vendor: step.vendor.map(|v| v.name().to_string()),
            model: step.model.clone(),
            subscription: step.vendor.map(|_| "default".to_string()),
            round: self.round,
            attempt,
            sha: Some(self.expected.clone()),
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

    /// Logs one of the engine's fixed gates. `failure` is why it failed. A
    /// failed gate stops the run. See `rework` for one the developer can fix.
    fn gate(&self, name: &str, failure: Option<String>) -> Result<Outcome> {
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
            round: self.round,
            attempt: 1,
            sha: Some(self.expected.clone()),
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
            Some(reason) => Outcome::Stop(format!("{name} gate: {reason}")),
            None => Outcome::Pass,
        })
    }

    /// A failed gate that goes back to the developer.
    fn rework(&self, name: &str, reason: String) -> Result<Outcome> {
        Ok(match self.gate(name, Some(reason))? {
            Outcome::Stop(reason) => Outcome::Rework(reason),
            other => other,
        })
    }

    /// Logs a step the engine does itself, such as a push or a merge.
    fn engine_step(&self, name: &str, verdict: &str, started: Instant) -> Result<()> {
        self.store.record(&StepRecord {
            ts: util::now(),
            repo: self.repo.clone(),
            issue: self.issue.number,
            run_id: self.run_id.clone(),
            step: name.to_string(),
            kind: "engine".to_string(),
            module: format!("engine/{name}"),
            vendor: None,
            model: None,
            subscription: None,
            round: self.round,
            attempt: 1,
            sha: Some(self.expected.clone()),
            exit_code: None,
            timed_out: false,
            verdict: verdict.to_string(),
            duration_ms: started.elapsed().as_millis() as u64,
            input: None,
            output: None,
            stdout: None,
            stderr: None,
            session_id: None,
        })
    }

    /// The part of a file name that says which round it belongs to.
    fn round_tag(&self) -> String {
        if self.round == 0 {
            String::new()
        } else {
            format!("-r{}", self.round)
        }
    }

    /// Calls an agent and parses its reply. An unusable reply gets one retry.
    /// `verdict_of` names what goes in the log for a usable reply. Returns
    /// the reply and the session it came from.
    fn ask<T: for<'de> Deserialize<'de>>(&self, index: usize, step: &Step, ask: &Ask, verdict_of: fn(&T) -> String) -> Result<Result<(T, Option<String>), String>> {
        let name = ask.name;
        let vendor = step.vendor.ok_or_else(|| anyhow!("step '{}' has no vendor", step.name))?;
        let schema = ask.schema.unwrap_or("{}");
        let access = step.def.access.unwrap_or(Access::ReadOnly);
        let timeout = Duration::from_secs(step.def.timeout_minutes.unwrap_or(30) * 60);
        let mut prompt = ask.prompt.to_string();
        let mut resume = ask.resume;
        let mut last_error = String::new();

        for attempt in 1..=2u32 {
            let how = if resume.is_some() { ", continuing its session" } else { "" };
            println!("  {name}: {} call, attempt {attempt}{how}", vendor.name());
            let stem = format!("{:02}-{name}{}-{attempt}", index + 1, self.round_tag());
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
                resume,
            })?;
            let parsed = match (&reply.output, &reply.error) {
                (Some(value), None) => serde_json::from_value::<T>(value.clone()).map_err(|e| format!("reply does not match the schema: {e}")),
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
                    println!("  {name}: replied in {}", seconds(reply.outcome.duration));
                    let session = reply.session_id.clone();
                    self.record(step, name, attempt, &verdict_of(&found), Evidence { outcome: Some(&reply.outcome), files, session: reply.session_id })?;
                    return Ok(Ok((found, session)));
                }
                Err(error) => {
                    println!("  {name}: unusable reply ({error})");
                    self.record(step, name, attempt, "invalid", Evidence { outcome: Some(&reply.outcome), files, session: reply.session_id })?;
                    // A timeout or a crash is not a format problem. Don't repeat it.
                    if reply.outcome.timed_out {
                        return Ok(Err(error));
                    }
                    if resume.is_some() && reply.output.is_none() {
                        // The session could not be continued. The prompt
                        // stands on its own, so start a new one.
                        println!("  {name}: the session could not be continued, starting a new one");
                        resume = None;
                    } else {
                        prompt = format!("{prompt}\n\n# Your previous reply was rejected\n\n{error}\n\nReply again with only the JSON object.");
                    }
                    last_error = error;
                }
            }
        }
        Ok(Err(format!("two unusable replies in a row. Last problem: {last_error}")))
    }

    /// The issue's flows as repo paths: those it adds, and those it keeps.
    fn flow_paths(&self, new: bool) -> Vec<String> {
        self.issue.marker.flows.iter().filter(|f| f.new == new).map(|f| flows::path(&f.file)).collect()
    }

    fn issue_values(&self) -> Vec<(&'static str, String)> {
        let marker = &self.issue.marker;
        let new = self.flow_paths(true);
        let existing = self.flow_paths(false);
        let mut flows: Vec<String> = new.iter().map(|f| format!("- `{f}`: written for this issue")).collect();
        flows.extend(existing.iter().map(|f| format!("- `{f}`: already there, and must keep passing")));
        let all: Vec<String> = new.iter().chain(existing.iter()).cloned().collect();
        vec![
            ("number", self.issue.number.to_string()),
            ("title", self.issue.title.clone()),
            ("summary", self.issue.summary.clone()),
            ("scope", bullets(&marker.scope, true)),
            ("accept", bullets(&marker.accept, false)),
            ("accessibility_ids", bullets(&self.ids, true)),
            ("flows", if flows.is_empty() { "None.".to_string() } else { flows.join("\n") }),
            ("new_flows", bullets(&new, true)),
            ("existing_flows", bullets(&existing, true)),
            ("issue_flows", bullets(&all, true)),
            (
                "acceptance_notes",
                match self.acceptance.as_ref().map(|a| a.report.notes.trim()).filter(|n| !n.is_empty()) {
                    Some(notes) => notes.to_string(),
                    None => "None.".to_string(),
                },
            ),
        ]
    }

    /// Records a step that made no call and ran no command.
    fn note(&self, step: &Step, verdict: &str) -> Result<()> {
        self.record(step, &step.name, 1, verdict, Evidence::default())
    }

    /// Reads flow files and applies the checks that need no simulator. `known`
    /// are the IDs a flow may use without them being in the code yet.
    fn flow_problems(&self, files: &[String], known: &[String]) -> Result<Option<(&'static str, String)>> {
        let mut unknown: Vec<String> = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(self.worktree.join(file)).with_context(|| format!("reading {file}"))?;
            let flow = match flows::read(&text) {
                Ok(flow) => flow,
                Err(problem) => return Ok(Some(("flow-format", format!("{file}: {problem}")))),
            };
            if !flow.banned.is_empty() {
                let found = flow.banned.join(", ");
                return Ok(Some(("plain-maestro", format!("{file} uses {found}. Flows may only use plain Maestro commands"))));
            }
            for id in flow.ids {
                if !known.contains(&id) && !unknown.contains(&id) && !git::in_code(&self.worktree, &format!("\"{id}\""), &[])? {
                    unknown.push(id);
                }
            }
        }
        if !unknown.is_empty() {
            return Ok(Some(("flow-ids", format!("flows use IDs that are neither listed for this issue nor in the code: {}", unknown.join(", ")))));
        }
        Ok(None)
    }

    /// Writes the issue's new acceptance flows, before any code exists.
    fn acceptance(&mut self, index: usize, step: &Step) -> Result<Outcome> {
        let new = self.flow_paths(true);
        if new.is_empty() {
            println!("  {}: skipped, the issue adds no flow", step.name);
            self.note(step, "skipped")?;
            return Ok(Outcome::Pass);
        }
        let values = self.issue_values();
        let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let prompt = render(step.prompt.as_deref().unwrap_or(""), &pairs);

        let ask = Ask { name: &step.name, prompt: &prompt, schema: step.schema.as_deref(), resume: None };
        let (report, _) = match self.ask::<AcceptanceReport>(index, step, &ask, |_| "replied".to_string())? {
            Ok(found) => found,
            Err(error) => return Ok(Outcome::Stop(error)),
        };

        if git::head(&self.worktree)? != self.expected {
            return self.gate("history", Some("the acceptance writer changed git history; only the engine commits".into()));
        }
        let changed = git::changed_files(&self.worktree, &self.expected)?;
        let missing: Vec<&String> = new.iter().filter(|f| !changed.contains(f)).collect();
        let extra: Vec<&String> = changed.iter().filter(|f| !new.contains(f)).collect();
        if !missing.is_empty() || !extra.is_empty() {
            return self.gate(
                "acceptance-files",
                Some(format!("the acceptance writer must write exactly {new:?}. Not written: {missing:?}. Not asked for: {extra:?}")),
            );
        }
        self.gate("acceptance-files", None)?;

        let invalid: Vec<&String> = report.added_ids.iter().filter(|id| !flows::valid_id(id)).collect();
        if !invalid.is_empty() {
            return self.gate("flow-ids", Some(format!("added IDs break the naming rule in AGENTS.md: {invalid:?}")));
        }
        let mut ids = self.issue.marker.accessibility_ids.clone();
        for id in &report.added_ids {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        if let Some((gate, problem)) = self.flow_problems(&new, &ids)? {
            return self.gate(gate, Some(problem));
        }
        self.gate("flow-ids", None)?;

        let subject = format!("test(acceptance): flows for #{}", self.issue.number);
        let sha = git::commit_all(&self.worktree, &subject, &format!("Issue #{}", self.issue.number))?;
        println!("  committed {} \"{subject}\": {}", &sha[..sha.len().min(10)], new.join(", "));
        if !report.added_ids.is_empty() {
            println!("  the acceptance writer added IDs: {}", report.added_ids.join(", "));
            let body = format!(
                "The acceptance writer added accessibility IDs beyond the plan's list. Development must implement them too.\n\n{}",
                bullets(&report.added_ids, true)
            );
            // A rerun that adds the same IDs does not comment again.
            let comments = format!("repos/{}/issues/{}/comments", self.repo, self.issue.number);
            let said = gh::api_list(&comments)
                .map(|list| list.iter().any(|c| c.get("body").and_then(Value::as_str).map(str::trim) == Some(body.trim())))
                .unwrap_or(false);
            if !said {
                if let Err(e) = gh::api("POST", &comments, Some(&json!({ "body": body }))) {
                    eprintln!("warning: could not comment on the issue: {e}");
                }
            }
        }
        self.ids = ids;
        self.expected = sha.clone();
        let vendor = step.vendor.map(|v| v.name()).unwrap_or("?").to_string();
        self.acceptance = Some(Acceptance { vendor, sha, report });
        Ok(Outcome::Pass)
    }

    /// The developer's turn. With `feedback`, this is a fix round: the
    /// developer continues its session and is told what failed.
    fn develop(&mut self, index: usize, step: &Step, feedback: Option<&Feedback>) -> Result<Outcome> {
        let mut values = self.issue_values();
        let prompt = match feedback {
            None => {
                let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
                render(step.prompt.as_deref().unwrap_or(""), &pairs)
            }
            Some(feedback) => {
                let state = match &self.head_sha {
                    Some(head) if *head == self.expected => format!("Your earlier work is committed as `{head}`. Build on it."),
                    Some(_) => "Your earlier work is committed, and the acceptance writer has since changed a flow at your request. Build on what is in the directory.".to_string(),
                    None => "Nothing of yours is committed yet. Your earlier changes are still in the directory, because a gate rejected them.".to_string(),
                };
                values.extend([
                    ("round", feedback.round.to_string()),
                    ("max_rounds", feedback.limit.to_string()),
                    ("failed_step", feedback.from.clone()),
                    ("state", state),
                    // Untrusted text, such as a log, goes last.
                    ("feedback", feedback.reason.clone()),
                ]);
                let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
                match step.fix_prompt.as_deref() {
                    Some(template) => render(template, &pairs),
                    None => format!(
                        "{}\n\n# Your earlier work did not pass, at the `{}` step\n\n{}\n\nFix it and reply again with the JSON object.",
                        render(step.prompt.as_deref().unwrap_or(""), &pairs),
                        feedback.from,
                        feedback.reason
                    ),
                }
            }
        };
        let resume = if feedback.is_some() { self.dev_session.clone() } else { None };
        let ask = Ask { name: &step.name, prompt: &prompt, schema: step.schema.as_deref(), resume: resume.as_deref() };
        let (report, session) = match self.ask::<DevReport>(index, step, &ask, |_| "replied".to_string())? {
            Ok(found) => found,
            Err(error) => return Ok(Outcome::Stop(error)),
        };
        if session.is_some() {
            self.dev_session = session;
        }
        if self.first_subject.is_none() {
            self.first_subject = Some(report.commit_subject.clone());
        }
        // A gate the developer can fix sends it back with the reason, and
        // with the failure it was already working on, which still stands.
        let earlier = match feedback {
            Some(f) if f.from != step.name => format!("\n\nThe failure you were fixing still stands:\n\n{}", f.reason),
            _ => String::new(),
        };

        // Fixed gates. None of these can be turned off by a workflow file.
        if git::head(&self.worktree)? != self.expected {
            return self.gate("history", Some("the developer changed git history; only the engine commits".into()));
        }
        let changed = git::changed_files(&self.worktree, &self.expected)?;
        if changed.is_empty() {
            // Trying again would get the same answer, so a person decides.
            return self.gate("changes", Some(format!("the developer changed no files. Its notes: {}", report.notes)));
        }
        let flows: Vec<&String> = changed.iter().filter(|f| f.starts_with(".maestro/")).collect();
        if !flows.is_empty() {
            return self.rework(
                "locked-flows",
                format!("acceptance flows are locked, but your change touches {flows:?}. Undo that, and use flow_change_request if a flow is wrong.{earlier}"),
            );
        }
        self.gate("locked-flows", None)?;
        let outside = outside_scope(&changed, &self.issue.marker.scope);
        if !outside.is_empty() {
            return self.rework(
                "scope",
                format!("you changed files outside the issue's scope: {}. Undo those changes.{earlier}", outside.join(", ")),
            );
        }
        self.gate("scope", None)?;
        println!("  scope gate: {} file(s), all inside scope", changed.len());
        // The IDs belong to elements this issue adds, so they must be in its scope.
        let mut absent: Vec<&String> = Vec::new();
        for id in &self.ids {
            if !git::in_code(&self.worktree, &format!("\"{id}\""), &self.issue.marker.scope)? {
                absent.push(id);
            }
        }
        if !absent.is_empty() {
            return self.rework(
                "ids-implemented",
                format!("these accessibility IDs are not string literals anywhere in the issue's scope: {absent:?}. Flows find elements by exactly these IDs.{earlier}"),
            );
        }
        self.gate("ids-implemented", None)?;

        // The developer's first commit holds the whole change, so it carries
        // the subject of the first attempt, whichever round it lands in.
        let first_commit = self.dev_subject.is_none();
        let proposed = if first_commit { self.first_subject.clone().unwrap_or_default() } else { report.commit_subject.clone() };
        let subject = commit_subject(&proposed, self.issue);
        let body = format!("Issue #{}", self.issue.number);
        let head = git::commit_all(&self.worktree, &subject, &body)?;
        println!("  committed {} \"{subject}\"", &head[..head.len().min(10)]);
        self.store.set_head(&self.repo, self.issue.number, &head)?;
        self.expected = head.clone();
        self.head_sha = Some(head.clone());
        self.dev_vendor = step.vendor.map(|v| v.name().to_string());
        if first_commit {
            self.dev_subject = Some(subject.clone());
        }
        if let Some(feedback) = feedback {
            self.fixes.push(Fix { round: self.round, from: feedback.from.clone(), why: first_line(&feedback.reason), commit: head, subject });
        }
        self.dev = Some(report);
        Ok(Outcome::Pass)
    }

    fn dev_report(&self) -> Result<String> {
        let dev = self.dev.as_ref().ok_or_else(|| anyhow!("the developer's report is missing"))?;
        Ok(format!(
            "{}\n\nFiles it lists: {}\n\nIts own verify run: {}\n\nNotes: {}",
            dev.summary,
            dev.files_changed.join(", "),
            dev.verify_ran,
            if dev.notes.trim().is_empty() { "none" } else { dev.notes.trim() }
        ))
    }

    /// The developer may not edit a flow. When it says one is wrong, the
    /// acceptance writer rules. `step` is the acceptance step.
    fn rule(&mut self, index: usize, step: &Step, request: &str) -> Result<Outcome> {
        let name = "flow-ruling";
        let head = self.head_sha.clone().ok_or_else(|| anyhow!("a ruling needs the developer's commit"))?;
        let mut values = self.issue_values();
        values.extend([("head_sha", head), ("dev_report", self.dev_report()?), ("request", request.trim().to_string())]);
        let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let prompt = render(step.ruling_prompt.as_deref().unwrap_or(""), &pairs);

        let ask = Ask { name, prompt: &prompt, schema: step.ruling_schema.as_deref(), resume: None };
        let (ruling, _) = match self.ask::<Ruling>(index, step, &ask, |r| r.decision.clone())? {
            Ok(found) => found,
            Err(error) => return Ok(Outcome::Stop(error)),
        };
        println!("  {name}: {} ({})", ruling.decision, ruling.reason.trim());
        if git::head(&self.worktree)? != self.expected {
            return self.gate("history", Some("the acceptance writer changed git history; only the engine commits".into()));
        }
        match ruling.decision.as_str() {
            "reject" => {
                let problem = self.same_commit("the acceptance writer")?;
                let reason = ruling.reason.trim().to_string();
                self.ruling = Some((ruling, None));
                Ok(match problem {
                    Some(problem) => Outcome::Stop(problem),
                    None => Outcome::Rework(format!(
                        "The acceptance writer rejected your flow change request, so the code must change to meet the flow as it is. Its reason: {reason}"
                    )),
                })
            }
            "accept" => {
                let allowed: Vec<String> = self.issue.marker.flows.iter().map(|f| flows::path(&f.file)).collect();
                let changed = git::changed_files(&self.worktree, &self.expected)?;
                let extra: Vec<&String> = changed.iter().filter(|f| !allowed.contains(f)).collect();
                if changed.is_empty() || !extra.is_empty() {
                    return self.gate(
                        "ruling-files",
                        Some(format!("an accepted flow change must edit the issue's flows and nothing else. Changed: {changed:?}")),
                    );
                }
                self.gate("ruling-files", None)?;
                // Development is finished, so a changed flow may only use IDs
                // that are in the code.
                let present: Vec<String> = changed.iter().filter(|f| self.worktree.join(f).exists()).cloned().collect();
                if let Some((gate, problem)) = self.flow_problems(&present, &[])? {
                    return self.gate(gate, Some(problem));
                }
                self.gate("flow-ids", None)?;
                let subject = format!("test(acceptance): accept flow change for #{}", self.issue.number);
                let sha = git::commit_all(&self.worktree, &subject, ruling.reason.trim())?;
                println!("  committed {} \"{subject}\": {}", &sha[..sha.len().min(10)], changed.join(", "));
                self.store.set_head(&self.repo, self.issue.number, &sha)?;
                self.expected = sha.clone();
                self.head_sha = Some(sha.clone());
                self.ruling = Some((ruling, Some(sha)));
                Ok(Outcome::Pass)
            }
            other => Ok(Outcome::Stop(format!("the acceptance writer returned an unknown decision '{other}'"))),
        }
    }

    /// Every step must leave the branch at the engine's latest commit, with
    /// nothing changed in the worktree.
    fn same_commit(&self, who: &str) -> Result<Option<String>> {
        let expected = &self.expected;
        if git::head(&self.worktree)? != *expected {
            return Ok(Some(format!("{who} moved the branch away from {expected}")));
        }
        let dirty = git::dirty(&self.worktree)?;
        if !dirty.is_empty() {
            return Ok(Some(format!("{who} left the worktree changed:\n{dirty}")));
        }
        Ok(None)
    }

    fn check(&mut self, index: usize, step: &Step) -> Result<Outcome> {
        let template = step.def.run.clone().unwrap_or_default();
        // A check on the new flows: they must fail before development.
        let red = template.contains("{{new_flows}}");
        let new = self.flow_paths(true);
        if red && new.is_empty() {
            println!("  {}: skipped, the issue adds no flow", step.name);
            self.note(step, "skipped")?;
            return Ok(Outcome::Pass);
        }
        // Flow names were checked to be plain file names before the run began.
        let quoted: Vec<String> = new.iter().map(|f| format!("'{f}'")).collect();
        let command = template.replace("{{new_flows}}", &quoted.join(" "));
        let judges_code = step.def.produces.iter().any(|p| p == "verify_log");
        let stem = format!("{:02}-{}{}", index + 1, step.name, self.round_tag());
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
        let files = [None, None, Some(stdout.as_path()), Some(stderr.as_path())];
        self.record(step, &step.name, 1, if passed { "pass" } else { "fail" }, Evidence { outcome: Some(&outcome), files, session: None })?;

        if !passed {
            let why = match outcome.exit_code {
                _ if outcome.timed_out => "timed out".to_string(),
                Some(code) => format!("exit code {code}"),
                None => "killed by a signal".to_string(),
            };
            if judges_code {
                self.verify = Some(format!("failed ({why})"));
            }
            println!("  {}: failed, {why}", step.name);
            let logs = format!("Full logs: {} and {}", stdout.display(), stderr.display());
            if outcome.timed_out {
                // A hang is rarely something a code change fixes.
                return Ok(Outcome::Stop(format!("`{command}` timed out. {logs}")));
            }
            let mut shown = tail(&stdout);
            let errors = tail(&stderr);
            if !errors.is_empty() {
                shown.push_str("\n\n--- stderr ---\n");
                shown.push_str(&errors);
            }
            return Ok(Outcome::Rework(format!(
                "`{command}` failed: {why}, on commit `{}`.\n\nThe end of its output:\n\n```\n{shown}\n```\n\nIf flows ran, Maestro's report and its screenshots of each failure are under `build/verify/` in your directory.\n\n{logs}",
                self.expected
            )));
        }
        if let Some(problem) = self.same_commit(&command)? {
            return Ok(Outcome::Stop(problem));
        }
        let result = format!("passed (exit 0, {})", seconds(outcome.duration));
        if red {
            self.red = Some(result.clone());
        }
        if judges_code {
            self.verify = Some(result);
        }
        println!("  {}: passed in {}", step.name, seconds(outcome.duration));
        Ok(Outcome::Pass)
    }

    /// What the reviewer is told about the acceptance flows.
    fn acceptance_note(&self) -> String {
        let Some(acceptance) = &self.acceptance else {
            return "No acceptance flows were written in this run.".to_string();
        };
        let mut out = format!(
            "Written before development by {} and committed as `{}`: {}. The developer may not edit them.",
            acceptance.vendor,
            acceptance.sha,
            self.flow_paths(true).join(", ")
        );
        match &self.red {
            Some(result) => out.push_str(&format!(" Before development, the check that they fail on the old app {result}.")),
            None => out.push_str(" They were not run against the old app."),
        }
        if let Some((ruling, Some(sha))) = &self.ruling {
            out.push_str(&format!(
                " After development the writer accepted a flow change request and changed the flows in `{sha}`: {}",
                ruling.reason.trim()
            ));
        }
        out
    }

    fn review(&mut self, index: usize, step: &Step) -> Result<Outcome> {
        let head = self.head_sha.clone().ok_or_else(|| anyhow!("review needs a commit"))?;
        // The branch's own changes. After the base branch was merged in, that
        // is no longer everything since the commit the branch started from.
        let from = git::git(&self.worktree, &["merge-base", &self.base_ref, &head]).unwrap_or_else(|_| self.base_sha.clone());
        let mut diff = git::diff(&self.worktree, &from, &head)?;
        if diff.len() > MAX_DIFF_CHARS {
            let cut = (0..=MAX_DIFF_CHARS).rev().find(|&i| diff.is_char_boundary(i)).unwrap_or(0);
            diff.truncate(cut);
            diff.push_str("\n[diff cut here: read the files for the rest]");
        }
        let dev_report = self.dev_report()?;
        let verify = self.verify.clone().unwrap_or_else(|| "not run".to_string());
        let mut values = self.issue_values();
        values.extend([
            ("head_sha", head.clone()),
            ("base_sha", from),
            ("verify_result", verify),
            ("acceptance", self.acceptance_note()),
            (
                "previous_review",
                match &self.earlier_review {
                    Some(earlier) => format!(
                        "An earlier commit, `{}`, was reviewed and these changes were asked for. The developer has worked on them since. Check each one, and don't ask for something new that the earlier review could have raised unless it is a bug or an unmet criterion.\n\n{}",
                        earlier.head_sha.trim(),
                        bullets(&earlier.items, false)
                    ),
                    None => "None. This is the first review.".to_string(),
                },
            ),
            ("dev_report", dev_report),
            ("diff", diff),
        ]);
        let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let prompt = render(step.prompt.as_deref().unwrap_or(""), &pairs);

        let ask = Ask { name: &step.name, prompt: &prompt, schema: step.schema.as_deref(), resume: None };
        let (verdict, _) = match self.ask::<Verdict>(index, step, &ask, |v| v.verdict.clone())? {
            Ok(found) => found,
            Err(error) => return Ok(Outcome::Stop(error)),
        };
        if let Some(problem) = self.same_commit("the reviewer")? {
            return Ok(Outcome::Stop(problem));
        }
        // One SHA for every signal: a verdict on another commit is worthless.
        let reviewed = verdict.head_sha.trim();
        if reviewed.len() < 7 || !head.starts_with(reviewed) {
            return Ok(Outcome::Stop(format!("the reviewer judged commit '{reviewed}', not {head}")));
        }
        let vendor = step.vendor.map(|v| v.name()).unwrap_or("?").to_string();
        println!("  {}: {} by {vendor}", step.name, verdict.verdict);
        let result = match verdict.verdict.as_str() {
            "approve" => Outcome::Pass,
            "changes" => {
                let items: Vec<String> = verdict.items.iter().enumerate().map(|(i, item)| format!("R{}: {}", i + 1, item.trim())).collect();
                self.earlier_review = Some(verdict.clone());
                Outcome::Rework(format!(
                    "The reviewer ({vendor}) asked for changes on commit `{head}`.\n\n{}\n\nFix each of these:\n\n{}",
                    verdict.summary.trim(),
                    bullets(&items, false)
                ))
            }
            other => Outcome::Stop(format!("the reviewer returned an unknown verdict '{other}'")),
        };
        self.review = Some((vendor, verdict));
        Ok(result)
    }

    /// Takes in the base branch when it has moved since the branch last
    /// contained it. The branch then holds a commit nothing has judged, so
    /// every check runs again. `name` is the step doing the asking.
    fn sync(&mut self, name: &str) -> Result<Outcome> {
        let started = Instant::now();
        git::git(&self.worktree, &["fetch", "--quiet", "origin"])?;
        if git::is_ancestor(&self.worktree, &self.base_ref, "HEAD")? {
            if name == "sync" {
                self.engine_step(name, "pass", started)?;
            }
            return Ok(Outcome::Pass);
        }
        let tip = git::git(&self.worktree, &["rev-parse", &self.base_ref])?;
        println!("  {name}: {} moved to {}, merging it in", self.base, &tip[..tip.len().min(10)]);
        let message = format!("merge: {} into {}", self.base, self.branch);
        match git::merge_in(&self.worktree, &self.base_ref, &message)? {
            Ok(sha) => {
                self.store.set_head(&self.repo, self.issue.number, &sha)?;
                self.expected = sha.clone();
                self.head_sha = Some(sha.clone());
                self.engine_step(name, "merged-base", started)?;
                Ok(Outcome::Again(format!("{} moved to {tip} and was merged in as {sha}", self.base)))
            }
            Err(conflicts) => {
                self.engine_step(name, "conflict", started)?;
                Ok(Outcome::Stop(format!(
                    "{} moved to {tip}, and merging it in conflicts in: {}. A person resolves this",
                    self.base,
                    conflicts.join(", ")
                )))
            }
        }
    }

    /// Pushes the branch and opens the pull request, or refreshes its body
    /// when it is already open.
    fn publish(&mut self) -> Result<Outcome> {
        let started = Instant::now();
        if let Err(e) = git::git(&self.worktree, &["push", "--quiet", "-u", "origin", &self.branch]) {
            self.engine_step("pr", "push-failed", started)?;
            return Ok(Outcome::Stop(format!("could not push {}: {e:#}", self.branch)));
        }
        self.pushed = Some(Instant::now());
        let body = self.pr_body();
        std::fs::write(self.run_dir.join(format!("pr-body{}.md", self.round_tag())), &body)?;
        match &self.pr {
            Some((number, _)) => {
                github::update_pr_body(&self.repo, *number, &body)?;
                println!("  pr: pushed {} to #{number}", &self.expected[..self.expected.len().min(10)]);
            }
            None => {
                let title = match &self.dev_subject {
                    Some(subject) => subject.clone(),
                    None => git::git(&self.worktree, &["log", "-1", "--format=%s"])?,
                };
                let (number, url) = match github::open_pr(&self.repo, &self.base, &self.branch, &title, &body) {
                    Ok(found) => found,
                    Err(e) => {
                        self.engine_step("pr", "open-failed", started)?;
                        return Ok(Outcome::Stop(format!("could not open the pull request: {e:#}")));
                    }
                };
                self.store.set_pr(&self.repo, self.issue.number, number, &url)?;
                println!("  pr: opened {url}");
                self.pr = Some((number, url));
            }
        }
        self.engine_step("pr", "pushed", started)?;
        Ok(Outcome::Pass)
    }

    /// Waits for GitHub's checks and review on the pull request's head
    /// commit. A check or a verdict for any other commit is ignored.
    fn github(&mut self) -> Result<Outcome> {
        let settings = self.github.clone().ok_or_else(|| anyhow!("the workflow has no github block"))?;
        let (pr, _) = self.pr.clone().ok_or_else(|| anyhow!("no pull request is open"))?;
        let head = self.head_sha.clone().ok_or_else(|| anyhow!("no commit to wait on"))?;
        let started = Instant::now();
        let wait = Duration::from_secs(settings.wait_minutes * 60);
        let seconds_from = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
        let poll = Duration::from_secs(seconds_from("FACTORY_POLL_SECONDS", 15));
        let settle = Duration::from_secs(seconds_from("FACTORY_SETTLE_SECONDS", 30));
        let mut shown = String::new();
        loop {
            let checks = github::checks(&self.repo, &head)?;
            let review = match settings.review_check {
                Some(_) => github::review_for(&self.repo, pr, &head)?,
                None => None,
            };
            let settled = self.pushed.is_none_or(|at| at.elapsed() >= settle);
            match github::judge(&settings, &checks, review.as_ref(), settled) {
                github::Standing::Waiting(what) => {
                    if what != shown {
                        println!("  github: waiting on {}, {what}", &head[..head.len().min(10)]);
                        shown = what.clone();
                    }
                    if started.elapsed() >= wait {
                        self.engine_step("github", "timeout", started)?;
                        return Ok(Outcome::Stop(format!(
                            "GitHub's checks on {head} did not finish in {} minutes: {what}",
                            settings.wait_minutes
                        )));
                    }
                    std::thread::sleep(poll);
                }
                github::Standing::Green => {
                    println!("  github: green on {} after {}", &head[..head.len().min(10)], seconds(started.elapsed()));
                    self.engine_step("github", "green", started)?;
                    return Ok(Outcome::Pass);
                }
                github::Standing::NoVerdict(why) => {
                    self.engine_step("github", "no-verdict", started)?;
                    return Ok(Outcome::Stop(why));
                }
                github::Standing::Failed { checks, review } => {
                    let mut text = format!("GitHub did not pass commit `{head}`, in pull request #{pr}.");
                    for check in &checks {
                        let log = github::job_log(&self.repo, check.id);
                        let file = self.run_dir.join(format!("github-{}{}.log", check.name.replace('/', "_"), self.round_tag()));
                        let _ = std::fs::write(&file, &log);
                        text.push_str(&format!(
                            "\n\n## The check `{}` ended as {}\n\n{}\n\nThe end of its log:\n\n```\n{}\n```",
                            check.name,
                            check.conclusion.as_deref().unwrap_or("unknown"),
                            check.url,
                            tail(&file)
                        ));
                    }
                    if let Some(review) = &review {
                        let items: Vec<String> = review.items.iter().enumerate().map(|(i, item)| format!("R{}: {}", i + 1, item.trim())).collect();
                        text.push_str(&format!(
                            "\n\n## The GitHub review asked for changes\n\n{}\n\nFix each of these:\n\n{}",
                            review.summary.trim(),
                            bullets(&items, false)
                        ));
                    }
                    println!("  github: failed on {}", &head[..head.len().min(10)]);
                    self.engine_step("github", "failed", started)?;
                    return Ok(Outcome::Rework(text));
                }
            }
        }
    }

    /// Merges the pull request, at the commit everything was judged on.
    fn merge(&mut self) -> Result<Outcome> {
        // The base branch may have moved while GitHub was checking.
        match self.sync("merge")? {
            Outcome::Pass => {}
            other => return Ok(other),
        }
        let started = Instant::now();
        let settings = self.github.clone().ok_or_else(|| anyhow!("the workflow has no github block"))?;
        let (pr, url) = self.pr.clone().ok_or_else(|| anyhow!("no pull request is open"))?;
        let head = self.head_sha.clone().ok_or_else(|| anyhow!("no commit to merge"))?;
        let title = format!("{} (#{pr})", self.dev_subject.clone().unwrap_or_else(|| self.issue.title.clone()));
        let message = format!("Closes #{}", self.issue.number);
        if let Err(e) = github::merge(&self.repo, pr, &head, &settings.merge, &title, &message) {
            self.engine_step("merge", "refused", started)?;
            return Ok(Outcome::Stop(format!("GitHub refused to merge #{pr} at {head}: {e:#}")));
        }
        println!("  merge: {url} merged into {} at {}", self.base, &head[..head.len().min(10)]);
        self.merged = true;
        self.engine_step("merge", "merged", started)?;
        Ok(Outcome::Pass)
    }

    fn pr_body(&self) -> String {
        let head = self.head_sha.as_deref().unwrap_or("?");
        let mut out = format!("Closes #{}\n\n## What changed\n\n", self.issue.number);
        if let Some(dev) = &self.dev {
            out.push_str(dev.summary.trim());
            out.push_str("\n\n");
        }
        if let Some(acceptance) = &self.acceptance {
            out.push_str("## Acceptance flows\n\n");
            out.push_str(&format!(
                "- Written before development by {}, commit `{}`: {}\n",
                acceptance.vendor,
                acceptance.sha,
                self.flow_paths(true).iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
            ));
            out.push_str(&format!("- {}\n", acceptance.report.summary.trim()));
            if let Some(red) = &self.red {
                out.push_str(&format!("- Check that the new flows fail before development: {red}\n"));
            }
            if !acceptance.report.added_ids.is_empty() {
                out.push_str(&format!("- IDs added beyond the plan's list: `{}`\n", acceptance.report.added_ids.join("`, `")));
            }
            if let Some((ruling, Some(sha))) = &self.ruling {
                out.push_str(&format!("- Flow change accepted after development, commit `{sha}`: {}\n", ruling.reason.trim()));
            }
            out.push('\n');
        }
        out.push_str("## Verification\n\n");
        out.push_str(&format!("- Commit verified: `{head}`\n"));
        out.push_str("- Scope gate: passed, every changed file is inside the issue's scope\n");
        out.push_str("- Locked flows: the developer's change touches no file in `.maestro/`\n");
        out.push_str(&format!("- `./verify.sh`, run by the engine on this commit: {}\n", self.verify.as_deref().unwrap_or("not run")));
        if let Some((vendor, verdict)) = &self.review {
            out.push_str(&format!("- Review ({vendor}): {} on `{}`\n", verdict.verdict, verdict.head_sha.trim()));
        }
        if !self.fixes.is_empty() {
            out.push_str("\n## Fix rounds\n\n");
            for fix in &self.fixes {
                out.push_str(&format!(
                    "{}. `{}` failed: {}\n   Fixed in `{}` \"{}\"\n",
                    fix.round,
                    fix.from,
                    fix.why.replace('`', "'"),
                    &fix.commit[..fix.commit.len().min(10)],
                    fix.subject
                ));
            }
        }
        out.push_str("\n## For a human to check\n\n");
        let mut notes: Vec<String> = Vec::new();
        if let Some(dev) = &self.dev {
            if !dev.notes.trim().is_empty() {
                notes.push(format!("Developer: {}", dev.notes.trim()));
            }
            if let Some(request) = dev.flow_change_request.as_deref().filter(|r| !r.trim().is_empty()) {
                let outcome = match &self.ruling {
                    Some((ruling, _)) => format!("{}ed by the acceptance writer", ruling.decision),
                    None => "not ruled on".to_string(),
                };
                notes.push(format!("Flow change requested ({outcome}): {}", request.trim()));
            }
        }
        if let Some(acceptance) = &self.acceptance {
            if !acceptance.report.notes.trim().is_empty() {
                notes.push(format!("Acceptance writer: {}", acceptance.report.notes.trim()));
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
    let Workflow { steps, mut on_fail, fix_rounds, github_fix_rounds, github } = workflow::parse(&workflow_text, &read_module)?;
    let acceptance_step = steps.iter().find(|s| s.def.role == Some(Role::Acceptance));

    // Flow names end up in file paths and in a shell command.
    if let Some(flow) = issue.marker.flows.iter().find(|f| !flows::safe_name(&f.file)) {
        bail!("issue #{} names the flow '{}', which is not a plain .yaml file name", issue.number, flow.file);
    }
    if acceptance_step.is_some() {
        for flow in &issue.marker.flows {
            let exists = read_base(&flows::path(&flow.file)).is_some();
            if flow.new && exists {
                bail!("issue #{} says it adds {}, but {base_ref} already has that file", issue.number, flows::path(&flow.file));
            }
            if !flow.new && !exists {
                bail!("issue #{} relies on {}, but {base_ref} does not have that file", issue.number, flows::path(&flow.file));
            }
        }
    }

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
        repo_dir: repo_dir.clone(),
        base: base.clone(),
        base_ref: base_ref.clone(),
        branch: branch.clone(),
        expected: base_sha.clone(),
        base_sha,
        head_sha: None,
        acceptance: None,
        red: None,
        ids: issue.marker.accessibility_ids.clone(),
        round: 0,
        fixes: Vec::new(),
        pr: None,
        github: github.clone(),
        pushed: None,
        merged: false,
        dev: None,
        dev_vendor: None,
        dev_subject: None,
        first_subject: None,
        dev_session: None,
        earlier_review: None,
        ruling: None,
        verify: None,
        review: None,
    };

    let dev_index = steps
        .iter()
        .position(|s| s.def.role == Some(Role::Dev))
        .ok_or_else(|| anyhow!("the workflow has no dev step"))?;
    // After the workflow's own steps come the engine's: take in a moved base
    // branch, push and open the pull request, wait on GitHub, merge. A repo
    // with no `github` block stops with the pull request open.
    let mut names: Vec<String> = steps.iter().map(|s| s.name.clone()).collect();
    if !opts.no_pr {
        names.extend(["sync".to_string(), "pr".to_string()]);
    }
    let github_from = names.len();
    let follows = github.is_some() && !opts.no_pr;
    if follows {
        names.push("github".to_string());
        on_fail.insert("github".to_string(), steps[dev_index].name.clone());
        if !opts.no_merge {
            names.push("merge".to_string());
        }
    }
    let limits = Limits { local: fix_rounds, github: github_fix_rounds, github_from };
    let (end, used) = rounds::drive(&names, dev_index, &on_fail, limits, &mut |index, turn| {
        let name = &names[index];
        run.round = turn.pass;
        if let Some(feedback) = turn.feedback {
            println!("  fix round {} of {}: back to {name} after {} failed", feedback.round, feedback.limit, feedback.from);
        }
        store.set_stage(&repo, issue.number, name, "running", None)?;
        let result = match steps.get(index) {
            None => match name.as_str() {
                "sync" => run.sync("sync"),
                "pr" => run.publish(),
                "github" => run.github(),
                "merge" => run.merge(),
                other => Err(anyhow!("'{other}' is not a step the engine knows")),
            },
            Some(step) => match (step.def.kind, step.def.role) {
                (Kind::Agent, Some(Role::Acceptance)) => run.acceptance(index, step),
                (Kind::Agent, Some(Role::Dev)) => run.develop(index, step, turn.feedback).and_then(|outcome| {
                    // A flow change request is ruled on before anything judges the code.
                    let request = run.dev.as_ref().and_then(|d| d.flow_change_request.clone()).filter(|r| !r.trim().is_empty());
                    match (outcome, request, acceptance_step.filter(|s| s.ruling_prompt.is_some())) {
                        (Outcome::Pass, Some(request), Some(writer)) => run.rule(index, writer, &request),
                        (outcome, _, _) => Ok(outcome),
                    }
                }),
                (Kind::Agent, Some(Role::Review)) => run.review(index, step),
                (Kind::Check, _) => run.check(index, step),
                _ => Err(anyhow!("step '{}' has a kind the engine can't run yet", step.name)),
            },
        };
        // An engine error stops the run like any failure no fix round can help.
        Ok(result.unwrap_or_else(|e| Outcome::Stop(format!("engine error: {e:#}"))))
    })?;
    let fix_rounds_used = used.local + used.github;
    let stopped = match end {
        End::Passed => None,
        End::Blocked { step, reason } => Some((step, reason)),
        End::OutOfRounds { step, reason, limit, github } => {
            let kind = if github { "GitHub fix round" } else { "fix round" };
            let rounds = if limit == 1 { format!("1 {kind} was") } else { format!("{limit} {kind}s were") };
            Some((step.clone(), format!("{rounds} used and '{step}' still fails. The last failure:\n{reason}")))
        }
    };
    if let Some((step, failure)) = stopped {
        store.set_stage(&repo, issue.number, &step, "failed", Some(&failure))?;
        label(&repo, issue.number, Some("blocked"), Some("in-progress"));
        println!("FAILED at {step}, after {fix_rounds_used} fix round(s): {failure}");
        if let Some((_, url)) = &run.pr {
            println!("  the pull request stays open: {url}");
        }
        println!("  a person decides from here. Worktree kept at {}", worktree.display());
        return Ok(false);
    }
    if fix_rounds_used > 0 {
        println!("  passed after {fix_rounds_used} fix round(s)");
    }
    if used.syncs > 0 {
        println!("  {base} moved {} time(s) during the run and was merged in", used.syncs);
    }

    let Some((_, url)) = run.pr.clone() else {
        store.set_stage(&repo, issue.number, "reviewed", "passed", None)?;
        println!("PASSED every step. No pull request opened (--no-pr)");
        return Ok(true);
    };
    if !run.merged {
        let (stage, status, note) = if follows {
            ("github", "green", "\n  GitHub's checks and review are green. Not merged (--no-merge)")
        } else {
            ("pr", "open", "")
        };
        store.set_stage(&repo, issue.number, stage, status, None)?;
        println!("PASSED every step. Pull request: {url}{note}");
        return Ok(true);
    }

    // Merged. The branch, the worktree and the label have done their work.
    // The run's files stay.
    store.set_stage(&repo, issue.number, "merged", "done", None)?;
    label(&repo, issue.number, None, Some("in-progress"));
    if let Err(e) = github::delete_branch(&repo, &branch) {
        eprintln!("warning: could not delete the branch {branch} on GitHub: {e}");
    }
    let cleanup = git::git(&run.repo_dir, &["worktree", "remove", "--force", &worktree.display().to_string()])
        .and_then(|_| git::git(&run.repo_dir, &["branch", "-D", &branch]));
    if let Err(e) = cleanup {
        eprintln!("warning: could not remove the worktree or its branch: {e:#}");
    }
    println!("MERGED into {base}: {url}");
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
        let mut shown_round = 0;
        for (step, vendor, attempt, verdict, exit_code, duration_ms, round) in store.steps(&row.repo, row.number, &row.run_id)? {
            if round != shown_round {
                println!("    back for pass {}", round + 1);
                shown_round = round;
            }
            let who = vendor.map(|v| format!(" [{v}]")).unwrap_or_default();
            let code = exit_code.map(|c| c.to_string()).unwrap_or_else(|| "-".into());
            println!("    {step}{who} attempt {attempt}: {verdict}, exit {code}, {:.1} s", duration_ms as f64 / 1000.0);
        }
        if let Some(failure) = &row.failure {
            println!("  failure: {}", first_line(failure));
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
    fn log_tails_keep_the_end() {
        let dir = std::env::temp_dir().join(format!("factory-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("log.txt");
        let text: String = (1..=500).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&file, text).unwrap();
        let shown = tail(&file);
        assert!(shown.ends_with("line 500"), "{shown}");
        assert_eq!(shown.lines().count(), LOG_TAIL_LINES);
        assert!(!shown.contains("line 380\n"));

        // One very long line is cut from the front, on a character boundary.
        std::fs::write(&file, "é".repeat(LOG_TAIL_CHARS)).unwrap();
        assert!(tail(&file).len() <= LOG_TAIL_CHARS);
        assert_eq!(tail(&dir.join("missing.txt")), "");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(first_line("\n\n  scope gate: x\nmore"), "scope gate: x");
        assert_eq!(first_line(&"a".repeat(200)).chars().count(), 163);
    }

    #[test]
    fn templates_are_filled() {
        let out = render("#{{number}} {{title}} / {{number}}", &[("number", "4"), ("title", "Shell")]);
        assert_eq!(out, "#4 Shell / 4");
    }
}
