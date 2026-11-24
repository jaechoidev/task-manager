//! `factory repo init`: repository settings a template can't carry.
//!
//! Every step converges, so running it again on an existing repo is safe.
//! It ends with a preflight that reads each setting back from GitHub.

use crate::gh::{self, ApiError};
use crate::labels::LABELS;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

const RS_MAIN_REVIEW: &str = "main: pull request with 1 approval";
const RS_MAIN_GUARD: &str = "main: no force push, no deletion, plan-validate";
const RS_DEV_GUARD: &str = "dev: no force push, no deletion";

/// The check `plan-validate.yml` reports. The job name must match.
pub const REQUIRED_CHECK: &str = "plan-validate";
/// GitHub Actions' app ID, so no other app can report the required check.
const ACTIONS_APP_ID: u64 = 15368;

const WORKFLOWS: &[&str] = &[".github/workflows/plan-validate.yml", ".github/workflows/plan-create.yml"];

pub struct Options {
    /// `owner/name` of the repo to create or converge.
    pub repo: String,
    /// `owner/name` of the template repo.
    pub template: Option<String>,
    pub collaborators: Vec<String>,
    pub description: Option<String>,
    /// Secrets the workflows need. They are checked, never written.
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub status: Status,
    pub name: String,
    pub detail: String,
}

fn step(message: &str) {
    println!("  {message}");
}

fn split(repo: &str) -> Result<(&str, &str)> {
    repo.split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .ok_or_else(|| anyhow!("'{repo}' must be written owner/name"))
}

/// Who may bypass the review rule on a pull request: the owner of a personal
/// repo, or the admin role in an organization.
fn bypass_actor(owner: &str) -> Result<Value> {
    let account = gh::api("GET", &format!("users/{owner}"), None)?;
    if account.get("type").and_then(Value::as_str) == Some("Organization") {
        return Ok(json!({ "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "pull_request" }));
    }
    let id = account.get("id").and_then(Value::as_u64).ok_or_else(|| anyhow!("no id for user {owner}"))?;
    Ok(json!({ "actor_id": id, "actor_type": "User", "bypass_mode": "pull_request" }))
}

fn rulesets(owner: &str) -> Result<Vec<Value>> {
    let branch = |name: &str| json!({ "ref_name": { "include": [format!("refs/heads/{name}")], "exclude": [] } });
    Ok(vec![
        json!({
            "name": RS_MAIN_REVIEW,
            "target": "branch",
            "enforcement": "active",
            "bypass_actors": [bypass_actor(owner)?],
            "conditions": branch("main"),
            "rules": [{
                "type": "pull_request",
                "parameters": {
                    "required_approving_review_count": 1,
                    "dismiss_stale_reviews_on_push": true,
                    "require_last_push_approval": true,
                    "require_code_owner_review": false,
                    "required_review_thread_resolution": false,
                    "allowed_merge_methods": ["merge", "squash", "rebase"]
                }
            }]
        }),
        // No bypass here: not even the owner merges a plan that fails validation.
        json!({
            "name": RS_MAIN_GUARD,
            "target": "branch",
            "enforcement": "active",
            "bypass_actors": [],
            "conditions": branch("main"),
            "rules": [
                { "type": "non_fast_forward" },
                { "type": "deletion" },
                {
                    "type": "required_status_checks",
                    "parameters": {
                        "strict_required_status_checks_policy": false,
                        "do_not_enforce_on_create": false,
                        "required_status_checks": [{ "context": REQUIRED_CHECK, "integration_id": ACTIONS_APP_ID }]
                    }
                }
            ]
        }),
        json!({
            "name": RS_DEV_GUARD,
            "target": "branch",
            "enforcement": "active",
            "bypass_actors": [],
            "conditions": branch("dev"),
            "rules": [{ "type": "non_fast_forward" }, { "type": "deletion" }]
        }),
    ])
}

fn exists(result: Result<Value, ApiError>) -> Result<Option<Value>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.is(404) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn init(opts: &Options) -> Result<Vec<Check>> {
    let (owner, name) = split(&opts.repo)?;
    let repo = &opts.repo;
    println!("repo init {repo}");

    // 1. The repository itself.
    if exists(gh::api("GET", &format!("repos/{repo}"), None))?.is_some() {
        step("repository already exists, settings will be converged");
    } else {
        let template = opts
            .template
            .as_deref()
            .ok_or_else(|| anyhow!("{repo} does not exist; pass --template owner/name to create it"))?;
        split(template)?;
        let mut request = json!({ "owner": owner, "name": name, "private": true, "include_all_branches": false });
        if let Some(description) = &opts.description {
            request["description"] = json!(description);
        }
        gh::api("POST", &format!("repos/{template}/generate"), Some(&request))
            .with_context(|| format!("creating {repo} from template {template}"))?;
        step(&format!("created private repository from {template}"));
    }

    // 2. Branches. A repo made from a template fills in a moment later.
    // Until the copy lands, GitHub answers 409 "Git Repository is empty".
    let mut main_sha = None;
    for _ in 0..30 {
        match gh::api("GET", &format!("repos/{repo}/git/ref/heads/main"), None) {
            Ok(reference) => {
                main_sha = reference.pointer("/object/sha").and_then(Value::as_str).map(str::to_string);
                break;
            }
            Err(e) if e.is(404) || e.is(409) => std::thread::sleep(Duration::from_secs(2)),
            Err(e) => return Err(e.into()),
        }
    }
    let main_sha = main_sha.ok_or_else(|| anyhow!("{repo} has no main branch after 60 seconds"))?;

    if exists(gh::api("GET", &format!("repos/{repo}/git/ref/heads/dev"), None))?.is_none() {
        let request = json!({ "ref": "refs/heads/dev", "sha": main_sha });
        gh::api("POST", &format!("repos/{repo}/git/refs"), Some(&request))?;
        step("created branch dev from main");
    }
    gh::api("PATCH", &format!("repos/{repo}"), Some(&json!({ "default_branch": "dev" })))?;
    step("default branch is dev");

    // 3. Rulesets, matched by name.
    let current = gh::api_list(&format!("repos/{repo}/rulesets"))?;
    for ruleset in rulesets(owner)? {
        let wanted = ruleset["name"].as_str().unwrap_or_default();
        let existing = current
            .iter()
            .find(|r| r.get("name").and_then(Value::as_str) == Some(wanted))
            .and_then(|r| r.get("id").and_then(Value::as_u64));
        match existing {
            Some(id) => gh::api("PUT", &format!("repos/{repo}/rulesets/{id}"), Some(&ruleset))?,
            None => gh::api("POST", &format!("repos/{repo}/rulesets"), Some(&ruleset))?,
        };
        step(&format!("ruleset \"{wanted}\" is active"));
    }

    // 4. Collaborators. A new one gets an invitation to accept.
    for login in &opts.collaborators {
        let path = format!("repos/{repo}/collaborators/{}", gh::encode(login));
        let reply = gh::api("PUT", &path, Some(&json!({ "permission": "push" })))?;
        if reply.is_null() {
            step(&format!("{login} already has access"));
        } else {
            step(&format!("invited {login} with write access"));
        }
    }

    // 5. Labels.
    for label in LABELS {
        let request = json!({ "name": label.name, "color": label.color, "description": label.description });
        match gh::api("POST", &format!("repos/{repo}/labels"), Some(&request)) {
            Ok(_) => {}
            Err(e) if e.is(422) => {
                let path = format!("repos/{repo}/labels/{}", gh::encode(label.name));
                let update = json!({ "color": label.color, "description": label.description });
                gh::api("PATCH", &path, Some(&update))?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    step(&format!("{} labels are in place", LABELS.len()));

    // 6. Actions: on, read-only token by default, and it can't approve pull
    // requests, so a workflow can never supply the required approval.
    let actions = json!({ "enabled": true, "allowed_actions": "all" });
    gh::api("PUT", &format!("repos/{repo}/actions/permissions"), Some(&actions))?;
    let workflow = json!({ "default_workflow_permissions": "read", "can_approve_pull_request_reviews": false });
    gh::api("PUT", &format!("repos/{repo}/actions/permissions/workflow"), Some(&workflow))?;
    step("Actions enabled with a read-only default token");

    preflight(opts)
}

fn has_rule<'a>(rules: &'a [Value], kind: &str) -> Option<&'a Value> {
    rules.iter().find(|r| r.get("type").and_then(Value::as_str) == Some(kind))
}

/// Reads every setting back from GitHub. Nothing is changed.
pub fn preflight(opts: &Options) -> Result<Vec<Check>> {
    let (owner, _) = split(&opts.repo)?;
    let repo = &opts.repo;
    let mut checks: Vec<Check> = Vec::new();
    let mut add = |ok: bool, name: &str, detail: String| {
        let status = if ok { Status::Pass } else { Status::Fail };
        checks.push(Check { status, name: name.to_string(), detail });
    };

    let Some(info) = exists(gh::api("GET", &format!("repos/{repo}"), None))? else {
        bail!("{repo} does not exist or is not visible to this account");
    };
    let private = info.get("private").and_then(Value::as_bool) == Some(true);
    add(private, "repository is private", format!("visibility: {}", info["visibility"].as_str().unwrap_or("?")));
    let default = info.get("default_branch").and_then(Value::as_str).unwrap_or("?");
    add(default == "dev", "default branch is dev", format!("default branch: {default}"));

    for branch in ["main", "dev"] {
        let found = exists(gh::api("GET", &format!("repos/{repo}/git/ref/heads/{branch}"), None))?.is_some();
        add(found, &format!("branch {branch} exists"), if found { "found".into() } else { "missing".into() });
    }

    // Rulesets as configured.
    let wanted = rulesets(owner)?;
    let summaries = gh::api_list(&format!("repos/{repo}/rulesets"))?;
    for ruleset in &wanted {
        let name = ruleset["name"].as_str().unwrap_or_default();
        let id = summaries
            .iter()
            .find(|r| r.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|r| r.get("id").and_then(Value::as_u64));
        let Some(id) = id else {
            add(false, &format!("ruleset \"{name}\""), "missing".into());
            continue;
        };
        let live = gh::api("GET", &format!("repos/{repo}/rulesets/{id}"), None)?;
        let mut problems: Vec<String> = Vec::new();
        if live.get("enforcement").and_then(Value::as_str) != Some("active") {
            problems.push(format!("enforcement is {}", live["enforcement"]));
        }
        if live.pointer("/conditions/ref_name/include") != ruleset.pointer("/conditions/ref_name/include") {
            problems.push("targets a different branch".into());
        }
        let live_rules = live.get("rules").and_then(Value::as_array).cloned().unwrap_or_default();
        for rule in ruleset["rules"].as_array().into_iter().flatten() {
            let kind = rule["type"].as_str().unwrap_or_default();
            let Some(found) = has_rule(&live_rules, kind) else {
                problems.push(format!("rule {kind} is missing"));
                continue;
            };
            // Every parameter we set must read back with the same value.
            for (param, value) in rule.get("parameters").and_then(Value::as_object).into_iter().flatten() {
                let got = found.pointer(&format!("/parameters/{param}"));
                let same = match (param.as_str(), got) {
                    ("required_status_checks", Some(got)) => {
                        got.pointer("/0/context") == value.pointer("/0/context")
                    }
                    (_, got) => got == Some(value),
                };
                if !same {
                    problems.push(format!("{kind}.{param} is {}", got.unwrap_or(&Value::Null)));
                }
            }
        }
        let wanted_bypass = ruleset["bypass_actors"].as_array().cloned().unwrap_or_default();
        let live_bypass = live.get("bypass_actors").and_then(Value::as_array).cloned().unwrap_or_default();
        if wanted_bypass != live_bypass {
            problems.push(format!("bypass list is {}", Value::Array(live_bypass)));
        }
        let detail = if problems.is_empty() { format!("active, id {id}") } else { problems.join("; ") };
        add(problems.is_empty(), &format!("ruleset \"{name}\""), detail);
    }

    // Rules as GitHub applies them to each branch. This is the proof they took.
    for (branch, kinds) in [
        ("main", vec!["pull_request", "non_fast_forward", "deletion", "required_status_checks"]),
        ("dev", vec!["non_fast_forward", "deletion"]),
    ] {
        let applied = gh::api_list(&format!("repos/{repo}/rules/branches/{branch}"))?;
        let missing: Vec<&str> = kinds.iter().copied().filter(|k| has_rule(&applied, k).is_none()).collect();
        let detail = if missing.is_empty() { kinds.join(", ") } else { format!("not applied: {}", missing.join(", ")) };
        add(missing.is_empty(), &format!("rules applied to {branch}"), detail);
    }

    // Collaborators.
    let members = gh::api_list(&format!("repos/{repo}/collaborators?affiliation=all"))?;
    let invited = gh::api_list(&format!("repos/{repo}/invitations"))?;
    let mut pending: Vec<String> = Vec::new();
    for login in &opts.collaborators {
        let is = |v: &Value, pointer: &str| {
            v.pointer(pointer).and_then(Value::as_str).is_some_and(|l| l.eq_ignore_ascii_case(login))
        };
        if let Some(member) = members.iter().find(|m| is(m, "/login")) {
            let can_push = member.pointer("/permissions/push").and_then(Value::as_bool) == Some(true);
            let role = member.get("role_name").and_then(Value::as_str).unwrap_or("?");
            add(can_push, &format!("{login} has write access"), format!("role: {role}"));
        } else if invited.iter().any(|i| is(i, "/invitee/login")) {
            pending.push(login.clone());
        } else {
            add(false, &format!("{login} has write access"), "not a collaborator and not invited".into());
        }
    }

    // Labels.
    let live_labels = gh::api_list(&format!("repos/{repo}/labels"))?;
    let missing: Vec<&str> = LABELS
        .iter()
        .map(|l| l.name)
        .filter(|name| !live_labels.iter().any(|l| l.get("name").and_then(Value::as_str) == Some(name)))
        .collect();
    let detail = if missing.is_empty() { format!("{} present", LABELS.len()) } else { format!("missing: {}", missing.join(", ")) };
    add(missing.is_empty(), "labels", detail);

    // Actions.
    let actions = gh::api("GET", &format!("repos/{repo}/actions/permissions"), None)?;
    let enabled = actions.get("enabled").and_then(Value::as_bool) == Some(true);
    add(enabled, "Actions enabled", format!("enabled: {enabled}"));
    let workflow = gh::api("GET", &format!("repos/{repo}/actions/permissions/workflow"), None)?;
    let token = workflow.get("default_workflow_permissions").and_then(Value::as_str).unwrap_or("?");
    let approves = workflow.get("can_approve_pull_request_reviews").and_then(Value::as_bool).unwrap_or(true);
    add(
        token == "read" && !approves,
        "Actions token is read-only and can't approve pull requests",
        format!("default: {token}, can approve: {approves}"),
    );

    // Workflow files, on the branch plan pull requests target.
    for file in WORKFLOWS {
        let found = exists(gh::api("GET", &format!("repos/{repo}/contents/{file}?ref=main"), None))?.is_some();
        add(found, &format!("{file} on main"), if found { "found".into() } else { "missing".into() });
    }

    // Things only a person can finish. These warn and don't fail.
    for login in pending {
        checks.push(Check {
            status: Status::Warn,
            name: format!("{login} has write access"),
            detail: "invited, not accepted yet".into(),
        });
    }
    if !opts.secrets.is_empty() {
        let reply = gh::api("GET", &format!("repos/{repo}/actions/secrets"), None)?;
        let names: Vec<&str> = reply
            .get("secrets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("name").and_then(Value::as_str))
            .collect();
        for secret in &opts.secrets {
            let found = names.contains(&secret.as_str());
            checks.push(Check {
                status: if found { Status::Pass } else { Status::Warn },
                name: format!("secret {secret}"),
                detail: if found { "set".into() } else { "missing; add it in the repository settings".into() },
            });
        }
    }

    Ok(checks)
}

pub fn passed(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.status != Status::Fail)
}

pub fn report(repo: &str, checks: &[Check]) -> String {
    let mut out = format!("preflight {repo}\n");
    for check in checks {
        let tag = match check.status {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        };
        out.push_str(&format!("  {tag}  {:<58} {}\n", check.name, check.detail));
    }
    let count = |s: Status| checks.iter().filter(|c| c.status == s).count();
    out.push_str(&format!(
        "preflight {}: {} passed, {} warnings, {} failed\n",
        if passed(checks) { "passed" } else { "FAILED" },
        count(Status::Pass),
        count(Status::Warn),
        count(Status::Fail)
    ));
    out
}
