//! What the engine reads from and does on GitHub once a pull request is
//! open: the checks on its head commit, the review workflow's verdict, and
//! the merge.
//!
//! Every signal is tied to one commit. A check or a verdict for another
//! commit says nothing about this one.

use crate::gh;
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};

pub const REVIEW_PREFIX: &str = "<!-- factory:review ";
const SUFFIX: &str = " -->";
/// Only a workflow in the repository can comment as this account.
const ACTIONS_BOT: &str = "github-actions[bot]";

/// The `github:` block of a workflow file.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// How long to wait for the checks on one commit.
    pub wait_minutes: u64,
    /// Checks that must appear on the commit before it can be judged.
    pub checks: Vec<String>,
    /// The check whose verdict comment is the GitHub review.
    pub review_check: Option<String>,
    /// `squash`, `merge` or `rebase`.
    pub merge: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// For a GitHub Actions job this is also the job's ID.
    pub id: u64,
    pub name: String,
    pub completed: bool,
    pub conclusion: Option<String>,
    pub url: String,
}

impl Check {
    fn passed(&self) -> bool {
        matches!(self.conclusion.as_deref(), Some("success" | "neutral" | "skipped"))
    }
}

/// The machine-readable line the review workflow puts first in its comment.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Review {
    pub verdict: String,
    /// The commit the workflow ran on. The workflow sets it, not the model.
    pub sha: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub items: Vec<String>,
}

pub fn parse_review(body: &str) -> Option<Review> {
    let start = body.find(REVIEW_PREFIX)? + REVIEW_PREFIX.len();
    let end = start + body[start..].find(SUFFIX)?;
    serde_json::from_str(&body[start..end]).ok()
}

/// Where one commit stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Standing {
    /// Not everything has reported yet. The text says what is missing.
    Waiting(String),
    Green,
    /// Checks failed, or the review asked for changes. The developer can act
    /// on both.
    Failed { checks: Vec<Check>, review: Option<Review> },
    /// The review check ended without a usable verdict for this commit. No
    /// code change fixes that.
    NoVerdict(String),
}

/// Judges one commit from its check runs and the review verdict found for it.
/// `settled` says the commit has been on GitHub long enough for checks to
/// have appeared, which matters only when none are required.
pub fn judge(settings: &Settings, checks: &[Check], review: Option<&Review>, settled: bool) -> Standing {
    // A rerun leaves two runs with one name. The newer one counts.
    let mut latest: Vec<&Check> = Vec::new();
    for check in checks {
        match latest.iter_mut().find(|c| c.name == check.name) {
            Some(found) if found.id < check.id => *found = check,
            Some(_) => {}
            None => latest.push(check),
        }
    }
    let mut required: Vec<&String> = settings.checks.iter().collect();
    if let Some(name) = &settings.review_check {
        if !required.contains(&name) {
            required.push(name);
        }
    }
    let missing: Vec<&str> = required.iter().filter(|name| !latest.iter().any(|c| c.name == ***name)).map(|n| n.as_str()).collect();
    if !missing.is_empty() {
        return Standing::Waiting(format!("no run yet for: {}", missing.join(", ")));
    }
    let running: Vec<&str> = latest.iter().filter(|c| !c.completed).map(|c| c.name.as_str()).collect();
    if !running.is_empty() {
        return Standing::Waiting(format!("still running: {}", running.join(", ")));
    }
    if latest.is_empty() && !settled {
        return Standing::Waiting("no check has appeared yet".to_string());
    }

    let review_name = settings.review_check.as_deref();
    let failed: Vec<Check> = latest.iter().filter(|c| Some(c.name.as_str()) != review_name && !c.passed()).map(|c| (*c).clone()).collect();
    let mut changes = None;
    if let Some(name) = review_name {
        match review {
            Some(found) if found.verdict == "approve" => {}
            Some(found) if found.verdict == "changes" => changes = Some(found.clone()),
            Some(found) => return Standing::NoVerdict(format!("the review comment has an unknown verdict '{}'", found.verdict)),
            None => {
                let check = latest.iter().find(|c| c.name == name);
                let how = match check.and_then(|c| c.conclusion.as_deref()) {
                    Some("success") => "passed without posting a verdict. Is its token secret set?".to_string(),
                    Some(other) => format!("ended as '{other}' without posting a verdict"),
                    None => "ended without a conclusion".to_string(),
                };
                let url = check.map(|c| c.url.as_str()).unwrap_or("");
                return Standing::NoVerdict(format!("the '{name}' check {how} {url}").trim().to_string());
            }
        }
    }
    if failed.is_empty() && changes.is_none() {
        Standing::Green
    } else {
        Standing::Failed { checks: failed, review: changes }
    }
}

pub fn checks(repo: &str, sha: &str) -> Result<Vec<Check>> {
    let value = gh::api("GET", &format!("repos/{repo}/commits/{sha}/check-runs?per_page=100"), None)?;
    let runs = value.get("check_runs").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(runs
        .iter()
        .map(|run| Check {
            id: run.get("id").and_then(Value::as_u64).unwrap_or(0),
            name: run.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            completed: run.get("status").and_then(Value::as_str) == Some("completed"),
            conclusion: run.get("conclusion").and_then(Value::as_str).map(str::to_string),
            url: run.get("html_url").and_then(Value::as_str).unwrap_or("").to_string(),
        })
        .collect())
}

/// The newest verdict the review workflow posted for exactly this commit.
/// A comment from anyone but the Actions bot is ignored, whatever it says.
pub fn review_for(repo: &str, pr: u64, sha: &str) -> Result<Option<Review>> {
    let comments = gh::api_list(&format!("repos/{repo}/issues/{pr}/comments"))?;
    Ok(comments
        .iter()
        .filter(|c| c.get("user").and_then(|u| u.get("login")).and_then(Value::as_str) == Some(ACTIONS_BOT))
        .filter_map(|c| c.get("body").and_then(Value::as_str).and_then(parse_review))
        .filter(|review| review.sha == sha)
        .next_back())
}

/// The log of a GitHub Actions job, or an empty string when there is none.
pub fn job_log(repo: &str, check_id: u64) -> String {
    gh::api_text(&format!("repos/{repo}/actions/jobs/{check_id}/logs")).unwrap_or_default()
}

/// Opens a pull request and returns its number and URL.
pub fn open_pr(repo: &str, base: &str, head: &str, title: &str, body: &str) -> Result<(u64, String)> {
    let value = gh::api("POST", &format!("repos/{repo}/pulls"), Some(&json!({ "title": title, "head": head, "base": base, "body": body })))?;
    let number = value.get("number").and_then(Value::as_u64).ok_or_else(|| anyhow!("GitHub returned no pull request number"))?;
    let url = value.get("html_url").and_then(Value::as_str).unwrap_or("").to_string();
    Ok((number, url))
}

/// The open pull request for a branch, when there is one already.
pub fn find_pr(repo: &str, branch: &str) -> Result<Option<(u64, String)>> {
    let owner = repo.split('/').next().unwrap_or("");
    let found = gh::api("GET", &format!("repos/{repo}/pulls?state=open&head={owner}:{}", gh::encode(branch)), None)?;
    Ok(found.as_array().and_then(|list| list.first()).and_then(|pr| {
        Some((pr.get("number")?.as_u64()?, pr.get("html_url").and_then(Value::as_str).unwrap_or("").to_string()))
    }))
}

pub fn is_merged(repo: &str, pr: u64) -> Result<bool> {
    let value = gh::api("GET", &format!("repos/{repo}/pulls/{pr}"), None)?;
    Ok(value.get("merged").and_then(Value::as_bool).unwrap_or(false))
}

/// Closes a pull request the engine opened, saying why.
pub fn close_pr(repo: &str, pr: u64, why: &str) -> Result<()> {
    gh::api("POST", &format!("repos/{repo}/issues/{pr}/comments"), Some(&json!({ "body": why })))?;
    gh::api("PATCH", &format!("repos/{repo}/pulls/{pr}"), Some(&json!({ "state": "closed" })))?;
    Ok(())
}

pub fn update_pr(repo: &str, pr: u64, title: &str, body: &str) -> Result<()> {
    gh::api("PATCH", &format!("repos/{repo}/pulls/{pr}"), Some(&json!({ "title": title, "body": body })))?;
    Ok(())
}

pub fn update_pr_body(repo: &str, pr: u64, body: &str) -> Result<()> {
    gh::api("PATCH", &format!("repos/{repo}/pulls/{pr}"), Some(&json!({ "body": body })))?;
    Ok(())
}

/// Merges the pull request, but only if its head is still `sha`.
pub fn merge(repo: &str, pr: u64, sha: &str, method: &str, title: &str, message: &str) -> Result<()> {
    let body = json!({ "merge_method": method, "sha": sha, "commit_title": title, "commit_message": message });
    gh::api("PUT", &format!("repos/{repo}/pulls/{pr}/merge"), Some(&body))?;
    Ok(())
}

pub fn delete_branch(repo: &str, branch: &str) -> Result<()> {
    gh::api("DELETE", &format!("repos/{repo}/git/refs/heads/{branch}"), None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(review: bool) -> Settings {
        Settings { wait_minutes: 20, checks: Vec::new(), review_check: review.then(|| "review".to_string()), merge: "squash".into() }
    }

    fn check(id: u64, name: &str, conclusion: Option<&str>) -> Check {
        Check { id, name: name.into(), completed: conclusion.is_some(), conclusion: conclusion.map(str::to_string), url: format!("https://example.invalid/{id}") }
    }

    fn review(verdict: &str) -> Review {
        Review { verdict: verdict.into(), sha: "abc".into(), summary: "s".into(), items: vec!["rename it".into()] }
    }

    #[test]
    fn the_marker_line_is_read_from_a_comment() {
        let body = "<!-- factory:review {\"v\":1,\"verdict\":\"changes\",\"sha\":\"abc\",\"summary\":\"two \\u003e one\",\"items\":[\"a -\\u002d b\"]} -->\nVerdict: changes\n";
        let found = parse_review(body).unwrap();
        assert_eq!((found.verdict.as_str(), found.sha.as_str()), ("changes", "abc"));
        assert_eq!(found.summary, "two > one");
        assert_eq!(found.items, vec!["a -- b"]);
        assert!(parse_review("Verdict: approve").is_none());
        assert!(parse_review("<!-- factory:review not json -->").is_none());
    }

    #[test]
    fn a_commit_waits_until_every_required_check_has_finished() {
        let s = settings(true);
        assert_eq!(judge(&s, &[], None, true), Standing::Waiting("no run yet for: review".into()));
        assert_eq!(judge(&s, &[check(1, "review", None)], None, true), Standing::Waiting("still running: review".into()));
        let mut with_build = s.clone();
        with_build.checks = vec!["build".into()];
        let only_review = [check(1, "review", Some("success"))];
        assert_eq!(judge(&with_build, &only_review, Some(&review("approve")), true), Standing::Waiting("no run yet for: build".into()));
    }

    #[test]
    fn approval_needs_a_verdict_not_a_green_check() {
        let s = settings(true);
        let green = [check(1, "review", Some("success"))];
        assert_eq!(judge(&s, &green, Some(&review("approve")), true), Standing::Green);
        // The check passed but nothing was posted for this commit.
        let Standing::NoVerdict(why) = judge(&s, &green, None, true) else { panic!() };
        assert!(why.contains("passed without posting a verdict"), "{why}");
        // The workflow itself broke, for example on a bad token.
        let Standing::NoVerdict(why) = judge(&s, &[check(1, "review", Some("failure"))], None, true) else { panic!() };
        assert!(why.contains("ended as 'failure' without posting a verdict"), "{why}");
    }

    #[test]
    fn changes_and_red_checks_go_back_to_the_developer_together() {
        let s = settings(true);
        // The review check is red by design when it asks for changes.
        let asked = [check(1, "review", Some("failure"))];
        let Standing::Failed { checks, review: found } = judge(&s, &asked, Some(&review("changes")), true) else { panic!() };
        assert!(checks.is_empty());
        assert_eq!(found.unwrap().items, vec!["rename it"]);

        let red = [check(1, "review", Some("success")), check(2, "build", Some("failure")), check(3, "lint", Some("skipped"))];
        let Standing::Failed { checks, review: found } = judge(&s, &red, Some(&review("approve")), true) else { panic!() };
        assert_eq!(checks.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["build"]);
        assert!(found.is_none());
    }

    #[test]
    fn a_rerun_replaces_the_earlier_run_of_the_same_check() {
        let s = settings(false);
        let runs = [check(5, "build", Some("failure")), check(9, "build", Some("success"))];
        assert_eq!(judge(&s, &runs, None, true), Standing::Green);
        let runs = [check(9, "build", Some("failure")), check(5, "build", Some("success"))];
        assert!(matches!(judge(&s, &runs, None, true), Standing::Failed { .. }));
    }

    #[test]
    fn a_repo_with_no_checks_is_green_once_settled() {
        let s = settings(false);
        assert_eq!(judge(&s, &[], None, false), Standing::Waiting("no check has appeared yet".into()));
        assert_eq!(judge(&s, &[], None, true), Standing::Green);
    }
}
