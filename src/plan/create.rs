//! Turns validated plans into GitHub issues, parent links and blocked-by links.
//!
//! Safe to rerun: every issue carries a marker with its plan ID and key, and
//! an issue or link that already exists is left alone. Existing issues are
//! never edited.

use super::model::{Dep, IssueSpec, PlanSet};
use super::validate::Report;
use crate::gh;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::time::Duration;

const MARKER_PREFIX: &str = "<!-- factory:issue ";
const MARKER_SUFFIX: &str = " -->";

/// First line of the PR comment that lists the created issues.
pub const COMMENT_MARKER: &str = "<!-- factory:plan-create -->";

/// The machine-readable first line of every issue body. The task manager
/// reads scope, acceptance criteria, flows and IDs from here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub v: u32,
    pub plan: String,
    /// Absent on the plan's parent issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accept: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<MarkerFlow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accessibility_ids: Vec<String>,
    /// `plan-id/key` or `#42`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkerFlow {
    pub file: String,
    #[serde(default)]
    pub new: bool,
}

pub fn marker_line(marker: &Marker) -> String {
    // Escaped so no text inside the JSON can end the HTML comment early.
    let json = serde_json::to_string(marker)
        .unwrap_or_default()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace("--", "-\\u002d");
    format!("{MARKER_PREFIX}{json}{MARKER_SUFFIX}")
}

pub fn parse_marker(body: &str) -> Option<Marker> {
    let start = body.find(MARKER_PREFIX)? + MARKER_PREFIX.len();
    let end = start + body[start..].find(MARKER_SUFFIX)?;
    serde_json::from_str(&body[start..end]).ok()
}

#[derive(Debug, Clone)]
pub struct RemoteIssue {
    /// Database ID, which the link endpoints want.
    pub id: u64,
    pub number: u64,
    pub body: String,
    pub url: String,
}

pub trait Github {
    /// Every issue in the repo, open or closed, without pull requests.
    fn list_issues(&self) -> Result<Vec<RemoteIssue>>;
    fn get_issue(&self, number: u64) -> Result<Option<RemoteIssue>>;
    fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<RemoteIssue>;
    fn sub_issue_numbers(&self, parent: u64) -> Result<Vec<u64>>;
    fn add_sub_issue(&self, parent: u64, child_id: u64) -> Result<()>;
    fn blocked_by_numbers(&self, number: u64) -> Result<Vec<u64>>;
    fn add_blocked_by(&self, number: u64, blocker_id: u64) -> Result<()>;
}

#[derive(Debug, Clone, Serialize)]
pub struct IssueOutcome {
    /// Empty for the parent issue.
    pub key: String,
    pub title: String,
    /// `None` in a dry run for an issue that does not exist yet.
    pub number: Option<u64>,
    pub url: Option<String>,
    pub created: bool,
    pub blocked_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanOutcome {
    pub id: String,
    pub path: String,
    pub parent: IssueOutcome,
    pub issues: Vec<IssueOutcome>,
    pub links_added: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub dry_run: bool,
    pub warnings: Vec<String>,
    pub plans: Vec<PlanOutcome>,
}

type Key = (String, Option<String>);

/// `selected` holds indexes into `set.plans`. All plans must have passed
/// validation, because a selected plan may refer to any of them.
// A lookup below can fail between the check and the insert, so the entry
// API does not fit.
#[allow(clippy::map_entry)]
pub fn create(
    set: &PlanSet,
    report: &Report,
    selected: &[usize],
    github: &dyn Github,
    dry_run: bool,
) -> Result<Outcome> {
    if !report.ok {
        bail!("plans did not pass validation; nothing was created");
    }

    let mut warnings = Vec::new();
    let mut known: HashMap<Key, RemoteIssue> = HashMap::new();
    for issue in github.list_issues()? {
        let Some(marker) = parse_marker(&issue.body) else { continue };
        let name = match &marker.key {
            Some(key) => format!("{}/{key}", marker.plan),
            None => marker.plan.clone(),
        };
        let slot = (marker.plan, marker.key);
        match known.get(&slot) {
            Some(first) => {
                let (keep, extra) = (first.number.min(issue.number), first.number.max(issue.number));
                warnings.push(format!("{name} exists twice, as #{keep} and #{extra}; using #{keep}"));
                if issue.number < first.number {
                    known.insert(slot, issue);
                }
            }
            None => {
                known.insert(slot, issue);
            }
        }
    }

    let selected_ids: HashSet<&str> = selected.iter().map(|&pi| report.plans[pi].id.as_str()).collect();

    // Everything outside the selected plans must already be on GitHub.
    // Checked before the first write so a bad reference creates nothing.
    let mut by_number: HashMap<u64, RemoteIssue> = HashMap::new();
    for &pi in selected {
        let id = &report.plans[pi].id;
        for spec in &set.plans[pi].file.issues {
            for raw in &spec.depends_on {
                match raw.parse().map_err(|e| anyhow!(e))? {
                    Dep::Issue(n) => {
                        if !by_number.contains_key(&n) {
                            let found = github
                                .get_issue(n)?
                                .ok_or_else(|| anyhow!("{id}: depends_on #{n} is not an issue in this repository"))?;
                            by_number.insert(n, found);
                        }
                    }
                    Dep::Cross { plan, key } if plan != *id && !selected_ids.contains(plan.as_str()) => {
                        if !known.contains_key(&(plan.clone(), Some(key.clone()))) {
                            bail!("{id}: depends_on {plan}/{key} has no issue yet; create plan {plan} first");
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Pass 1: issues. Parents first, then children in dependency order.
    let mut created: HashSet<Key> = HashSet::new();
    for &pi in selected {
        let plan = &report.plans[pi];
        let file = &set.plans[pi].file;
        let parent_slot: Key = (plan.id.clone(), None);
        if !known.contains_key(&parent_slot) {
            created.insert(parent_slot.clone());
            if !dry_run {
                let body = parent_body(&plan.id, &plan.path, file.summary.as_deref(), &plan.batches);
                let issue = github.create_issue(&plan.title, &body, &[])?;
                known.insert(parent_slot, issue);
            }
        }
        for key in plan.batches.iter().flatten() {
            let slot: Key = (plan.id.clone(), Some(key.clone()));
            if known.contains_key(&slot) {
                continue;
            }
            created.insert(slot.clone());
            if dry_run {
                continue;
            }
            let spec = spec_for(file.issues.as_slice(), key)?;
            let deps = resolve_deps(&plan.id, spec)?;
            let body = issue_body(&plan.id, &plan.path, spec, &deps, &known);
            let title = spec.title.as_deref().unwrap_or(key).trim();
            let issue = github.create_issue(title, &body, &spec.labels)?;
            known.insert(slot, issue);
        }
    }

    // Pass 2: links. An issue created in this run has none yet, so its
    // current links are only read for issues that already existed.
    let mut plans = Vec::new();
    for &pi in selected {
        let plan = &report.plans[pi];
        let file = &set.plans[pi].file;
        let parent_slot: Key = (plan.id.clone(), None);
        let parent = known.get(&parent_slot);
        let mut links_added = 0;

        let current_children: Vec<u64> = match parent {
            Some(p) if !created.contains(&parent_slot) => github.sub_issue_numbers(p.number)?,
            _ => Vec::new(),
        };

        let mut issues = Vec::new();
        for key in plan.batches.iter().flatten() {
            let slot: Key = (plan.id.clone(), Some(key.clone()));
            let spec = spec_for(file.issues.as_slice(), key)?;
            let deps = resolve_deps(&plan.id, spec)?;
            let issue = known.get(&slot);

            if let (Some(parent), Some(issue)) = (parent, issue) {
                if !dry_run && !current_children.contains(&issue.number) {
                    github.add_sub_issue(parent.number, issue.id)?;
                    links_added += 1;
                }
            }

            let mut blocked_by = Vec::new();
            let current: Vec<u64> = match issue {
                Some(i) if !dry_run && !deps.is_empty() && !created.contains(&slot) => {
                    github.blocked_by_numbers(i.number)?
                }
                _ => Vec::new(),
            };
            for dep in &deps {
                let blocker = match dep {
                    ResolvedDep::Planned { plan, key } => known.get(&(plan.clone(), Some(key.clone()))),
                    ResolvedDep::Number(n) => by_number.get(n),
                };
                match blocker {
                    Some(blocker) => {
                        blocked_by.push(format!("#{}", blocker.number));
                        if let (Some(issue), false) = (issue, dry_run) {
                            if !current.contains(&blocker.number) {
                                github.add_blocked_by(issue.number, blocker.id)?;
                                links_added += 1;
                            }
                        }
                    }
                    None if dry_run => blocked_by.push(dep.name()),
                    None => bail!("{}/{key}: {} has no issue to link to", plan.id, dep.name()),
                }
            }

            issues.push(IssueOutcome {
                key: key.clone(),
                title: spec.title.clone().unwrap_or_default().trim().to_string(),
                number: issue.map(|i| i.number),
                url: issue.map(|i| i.url.clone()),
                created: created.contains(&slot),
                blocked_by,
            });
        }

        plans.push(PlanOutcome {
            id: plan.id.clone(),
            path: plan.path.clone(),
            parent: IssueOutcome {
                key: String::new(),
                title: plan.title.clone(),
                number: parent.map(|p| p.number),
                url: parent.map(|p| p.url.clone()),
                created: created.contains(&parent_slot),
                blocked_by: Vec::new(),
            },
            issues,
            links_added,
        });
    }

    Ok(Outcome { dry_run, warnings, plans })
}

enum ResolvedDep {
    Planned { plan: String, key: String },
    Number(u64),
}

impl ResolvedDep {
    fn name(&self) -> String {
        match self {
            ResolvedDep::Planned { plan, key } => format!("{plan}/{key}"),
            ResolvedDep::Number(n) => format!("#{n}"),
        }
    }
}

fn spec_for<'a>(issues: &'a [IssueSpec], key: &str) -> Result<&'a IssueSpec> {
    issues
        .iter()
        .find(|i| i.key.as_deref().map(str::trim) == Some(key))
        .ok_or_else(|| anyhow!("issue '{key}' is missing from its plan file"))
}

fn resolve_deps(plan_id: &str, spec: &IssueSpec) -> Result<Vec<ResolvedDep>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for raw in &spec.depends_on {
        let dep = match raw.parse().map_err(|e| anyhow!(e))? {
            Dep::Local(key) => ResolvedDep::Planned { plan: plan_id.to_string(), key },
            Dep::Cross { plan, key } => ResolvedDep::Planned { plan, key },
            Dep::Issue(n) => ResolvedDep::Number(n),
        };
        if seen.insert(dep.name()) {
            out.push(dep);
        }
    }
    Ok(out)
}

fn bullet_list(out: &mut String, heading: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(out, "## {heading}\n");
    for item in items {
        let _ = writeln!(out, "- {item}");
    }
    out.push('\n');
}

fn issue_body(
    plan_id: &str,
    path: &str,
    spec: &IssueSpec,
    deps: &[ResolvedDep],
    known: &HashMap<Key, RemoteIssue>,
) -> String {
    let key = spec.key.clone().unwrap_or_default().trim().to_string();
    let marker = Marker {
        v: 1,
        plan: plan_id.to_string(),
        key: Some(key.clone()),
        scope: spec.scope.iter().map(|s| s.trim().to_string()).collect(),
        accept: spec.accept.iter().map(|s| s.trim().to_string()).collect(),
        flows: spec
            .flows
            .iter()
            .map(|f| MarkerFlow { file: f.file().trim().to_string(), new: f.is_new() })
            .collect(),
        accessibility_ids: spec.accessibility_ids.clone(),
        depends_on: deps.iter().map(ResolvedDep::name).collect(),
    };

    let mut out = marker_line(&marker);
    out.push_str("\n\n");
    if let Some(summary) = spec.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let _ = writeln!(out, "{summary}\n");
    }
    let code = |s: &String| format!("`{}`", s.trim());
    bullet_list(&mut out, "Scope", &spec.scope.iter().map(code).collect::<Vec<_>>());
    bullet_list(
        &mut out,
        "Acceptance criteria",
        &spec.accept.iter().map(|a| format!("[ ] {}", a.trim())).collect::<Vec<_>>(),
    );
    bullet_list(
        &mut out,
        "Maestro flows",
        &spec
            .flows
            .iter()
            .map(|f| format!("`{}`{}", f.file().trim(), if f.is_new() { " (new, written before development)" } else { "" }))
            .collect::<Vec<_>>(),
    );
    bullet_list(&mut out, "Accessibility IDs", &spec.accessibility_ids.iter().map(code).collect::<Vec<_>>());
    bullet_list(
        &mut out,
        "Depends on",
        &deps
            .iter()
            .map(|dep| match dep {
                ResolvedDep::Number(n) => format!("#{n}"),
                ResolvedDep::Planned { plan, key } => match known.get(&(plan.clone(), Some(key.clone()))) {
                    Some(issue) => format!("#{} (`{plan}/{key}`)", issue.number),
                    None => format!("`{plan}/{key}`"),
                },
            })
            .collect::<Vec<_>>(),
    );
    let _ = writeln!(out, "---");
    let _ = writeln!(
        out,
        "Plan `{plan_id}`, key `{key}`, created from `{path}`. The first line of this issue is read by `factory`, so leave it as it is."
    );
    out
}

fn parent_body(plan_id: &str, path: &str, summary: Option<&str>, batches: &[Vec<String>]) -> String {
    let marker = Marker {
        v: 1,
        plan: plan_id.to_string(),
        key: None,
        scope: Vec::new(),
        accept: Vec::new(),
        flows: Vec::new(),
        accessibility_ids: Vec::new(),
        depends_on: Vec::new(),
    };
    let mut out = marker_line(&marker);
    out.push_str("\n\n");
    if let Some(summary) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        let _ = writeln!(out, "{summary}\n");
    }
    let _ = writeln!(out, "## Batches\n");
    for (i, batch) in batches.iter().enumerate() {
        let keys: Vec<String> = batch.iter().map(|k| format!("`{k}`")).collect();
        let _ = writeln!(out, "{}. {}", i + 1, keys.join(", "));
    }
    let _ = writeln!(out, "\n---");
    let _ = writeln!(
        out,
        "Parent issue for plan `{plan_id}`, created from `{path}`. The first line of this issue is read by `factory`, so leave it as it is."
    );
    out
}

fn counts(plan: &PlanOutcome) -> (usize, usize) {
    let all = std::iter::once(&plan.parent).chain(plan.issues.iter());
    let created = all.clone().filter(|i| i.created).count();
    (created, all.count() - created)
}

fn summary_line(outcome: &Outcome, plan: &PlanOutcome) -> String {
    let (created, existing) = counts(plan);
    if outcome.dry_run {
        format!("{created} would be created, {existing} already exist")
    } else {
        format!("{created} created, {existing} already existed, {} links added", plan.links_added)
    }
}

fn number_text(issue: &IssueOutcome) -> String {
    issue.number.map_or_else(|| "(new)".to_string(), |n| format!("#{n}"))
}

fn status_text(outcome: &Outcome, issue: &IssueOutcome) -> &'static str {
    match (issue.created, outcome.dry_run) {
        (true, true) => "would be created",
        (true, false) => "created",
        (false, _) => "already existed",
    }
}

pub fn text(outcome: &Outcome) -> String {
    let mut out = String::new();
    for warning in &outcome.warnings {
        let _ = writeln!(out, "WARN {warning}");
    }
    for plan in &outcome.plans {
        let _ = writeln!(out, "plan {}: {}", plan.id, summary_line(outcome, plan));
        for issue in std::iter::once(&plan.parent).chain(plan.issues.iter()) {
            let key = if issue.key.is_empty() { "(parent)" } else { issue.key.as_str() };
            let blocked = if issue.blocked_by.is_empty() {
                String::new()
            } else {
                format!("   blocked by {}", issue.blocked_by.join(", "))
            };
            let _ = writeln!(
                out,
                "  {:<6} {:<16} {}{}",
                number_text(issue),
                key,
                status_text(outcome, issue),
                blocked
            );
        }
    }
    if outcome.plans.is_empty() {
        out.push_str("no plans to create\n");
    }
    out
}

pub fn markdown(outcome: &Outcome) -> String {
    let mut out = format!("{COMMENT_MARKER}\n");
    for plan in &outcome.plans {
        let _ = writeln!(out, "## Issues for plan `{}`\n", plan.id);
        let _ = writeln!(out, "{}. File: `{}`\n", summary_line(outcome, plan), plan.path);
        out.push_str("| Key | Issue | Title | Status | Blocked by |\n| --- | --- | --- | --- | --- |\n");
        for issue in std::iter::once(&plan.parent).chain(plan.issues.iter()) {
            let key = if issue.key.is_empty() { "(parent)".to_string() } else { format!("`{}`", issue.key) };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                key,
                number_text(issue),
                issue.title.replace('|', "\\|"),
                status_text(outcome, issue),
                issue.blocked_by.join(", ")
            );
        }
        out.push('\n');
    }
    if !outcome.warnings.is_empty() {
        out.push_str("**Warnings**\n\n");
        for warning in &outcome.warnings {
            let _ = writeln!(out, "- {warning}");
        }
    }
    out
}

/// The real GitHub, through `gh api`.
pub struct GhIssues {
    pub repo: String,
}

impl GhIssues {
    fn remote(value: &serde_json::Value) -> Option<RemoteIssue> {
        if value.get("pull_request").is_some() {
            return None;
        }
        Some(RemoteIssue {
            id: value.get("id")?.as_u64()?,
            number: value.get("number")?.as_u64()?,
            body: value.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string(),
            url: value.get("html_url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
        })
    }

    fn numbers(values: Vec<serde_json::Value>) -> Vec<u64> {
        values.iter().filter_map(|v| v.get("number").and_then(|n| n.as_u64())).collect()
    }

    /// GitHub asks for a pause between writes, or it starts refusing them.
    fn pace() {
        std::thread::sleep(Duration::from_millis(1000));
    }
}

impl Github for GhIssues {
    fn list_issues(&self) -> Result<Vec<RemoteIssue>> {
        let values = gh::api_list(&format!("repos/{}/issues?state=all", self.repo))?;
        Ok(values.iter().filter_map(Self::remote).collect())
    }

    fn get_issue(&self, number: u64) -> Result<Option<RemoteIssue>> {
        match gh::api("GET", &format!("repos/{}/issues/{number}", self.repo), None) {
            Ok(value) => Ok(Self::remote(&value)),
            Err(e) if e.is(404) || e.is(410) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<RemoteIssue> {
        Self::pace();
        let request = json!({ "title": title, "body": body, "labels": labels });
        let value = gh::api("POST", &format!("repos/{}/issues", self.repo), Some(&request))?;
        Self::remote(&value).ok_or_else(|| anyhow!("GitHub returned an issue without an id"))
    }

    fn sub_issue_numbers(&self, parent: u64) -> Result<Vec<u64>> {
        Ok(Self::numbers(gh::api_list(&format!("repos/{}/issues/{parent}/sub_issues", self.repo))?))
    }

    fn add_sub_issue(&self, parent: u64, child_id: u64) -> Result<()> {
        Self::pace();
        let request = json!({ "sub_issue_id": child_id });
        gh::api("POST", &format!("repos/{}/issues/{parent}/sub_issues", self.repo), Some(&request))?;
        Ok(())
    }

    fn blocked_by_numbers(&self, number: u64) -> Result<Vec<u64>> {
        let path = format!("repos/{}/issues/{number}/dependencies/blocked_by", self.repo);
        Ok(Self::numbers(gh::api_list(&path)?))
    }

    fn add_blocked_by(&self, number: u64, blocker_id: u64) -> Result<()> {
        Self::pace();
        let request = json!({ "issue_id": blocker_id });
        let path = format!("repos/{}/issues/{number}/dependencies/blocked_by", self.repo);
        gh::api("POST", &path, Some(&request))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::model::{parse_str, LoadedPlan};
    use crate::plan::validate::{validate, Options};
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[derive(Default)]
    struct Fake {
        issues: RefCell<Vec<RemoteIssue>>,
        children: RefCell<HashMap<u64, Vec<u64>>>,
        blockers: RefCell<HashMap<u64, Vec<u64>>>,
        writes: RefCell<usize>,
    }

    impl Fake {
        fn number_of(&self, id: u64) -> u64 {
            id - 1000
        }
    }

    impl Github for Fake {
        fn list_issues(&self) -> Result<Vec<RemoteIssue>> {
            Ok(self.issues.borrow().clone())
        }
        fn get_issue(&self, number: u64) -> Result<Option<RemoteIssue>> {
            Ok(self.issues.borrow().iter().find(|i| i.number == number).cloned())
        }
        fn create_issue(&self, _title: &str, body: &str, _labels: &[String]) -> Result<RemoteIssue> {
            *self.writes.borrow_mut() += 1;
            let number = self.issues.borrow().len() as u64 + 1;
            let issue = RemoteIssue { id: number + 1000, number, body: body.to_string(), url: format!("u/{number}") };
            self.issues.borrow_mut().push(issue.clone());
            Ok(issue)
        }
        fn sub_issue_numbers(&self, parent: u64) -> Result<Vec<u64>> {
            Ok(self.children.borrow().get(&parent).cloned().unwrap_or_default())
        }
        fn add_sub_issue(&self, parent: u64, child_id: u64) -> Result<()> {
            *self.writes.borrow_mut() += 1;
            self.children.borrow_mut().entry(parent).or_default().push(self.number_of(child_id));
            Ok(())
        }
        fn blocked_by_numbers(&self, number: u64) -> Result<Vec<u64>> {
            Ok(self.blockers.borrow().get(&number).cloned().unwrap_or_default())
        }
        fn add_blocked_by(&self, number: u64, blocker_id: u64) -> Result<()> {
            *self.writes.borrow_mut() += 1;
            self.blockers.borrow_mut().entry(number).or_default().push(self.number_of(blocker_id));
            Ok(())
        }
    }

    const PLAN: &str = r#"
plan: water-v1
title: Water tracker
issues:
  - {key: entry-ui, title: Entry, scope: [Sources/UI/Entry/], accept: ["Tap --> total changes"], depends_on: [model]}
  - {key: model, title: Model, scope: [Sources/Model/], accept: [x], labels: ["size:S"]}
  - {key: history-ui, title: History, scope: [Sources/UI/History/], accept: [x], depends_on: [model, entry-ui]}
"#;

    fn load(files: &[(&str, &str)]) -> (PlanSet, Report) {
        let mut set = PlanSet::default();
        for (name, text) in files {
            let file = parse_str(text).expect("plan parses");
            set.plans.push(LoadedPlan { path: PathBuf::from(format!("plans/{name}.yaml")), file });
        }
        let report = validate(&set, &Options { flows_dir: PathBuf::from("/nonexistent") });
        assert!(report.ok, "{:?}", report.plans.iter().map(|p| &p.errors).collect::<Vec<_>>());
        (set, report)
    }

    #[test]
    fn first_run_creates_issues_and_links() {
        let (set, report) = load(&[("water-v1", PLAN)]);
        let fake = Fake::default();
        let outcome = create(&set, &report, &[0], &fake, false).unwrap();
        let plan = &outcome.plans[0];

        // Parent, then model before the two issues that depend on it.
        assert_eq!(plan.parent.number, Some(1));
        let order: Vec<(&str, Option<u64>)> = plan.issues.iter().map(|i| (i.key.as_str(), i.number)).collect();
        assert_eq!(order, vec![("model", Some(2)), ("entry-ui", Some(3)), ("history-ui", Some(4))]);
        assert_eq!(fake.children.borrow()[&1], vec![2, 3, 4]);
        assert_eq!(fake.blockers.borrow()[&3], vec![2]);
        assert_eq!(fake.blockers.borrow()[&4], vec![2, 3]);
        assert_eq!(plan.links_added, 6);
        assert_eq!(counts(plan), (4, 0));
    }

    #[test]
    fn second_run_creates_nothing() {
        let (set, report) = load(&[("water-v1", PLAN)]);
        let fake = Fake::default();
        create(&set, &report, &[0], &fake, false).unwrap();
        let writes = *fake.writes.borrow();

        let outcome = create(&set, &report, &[0], &fake, false).unwrap();
        assert_eq!(*fake.writes.borrow(), writes);
        assert_eq!(counts(&outcome.plans[0]), (0, 4));
        assert_eq!(outcome.plans[0].links_added, 0);
        assert_eq!(outcome.plans[0].issues[2].blocked_by, vec!["#2", "#3"]);
    }

    #[test]
    fn rerun_repairs_a_missing_link() {
        let (set, report) = load(&[("water-v1", PLAN)]);
        let fake = Fake::default();
        create(&set, &report, &[0], &fake, false).unwrap();
        fake.blockers.borrow_mut().insert(4, vec![2]);
        fake.children.borrow_mut().insert(1, vec![2, 4]);

        let outcome = create(&set, &report, &[0], &fake, false).unwrap();
        assert_eq!(outcome.plans[0].links_added, 2);
        assert_eq!(fake.blockers.borrow()[&4], vec![2, 3]);
        assert_eq!(fake.issues.borrow().len(), 4);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (set, report) = load(&[("water-v1", PLAN)]);
        let fake = Fake::default();
        let outcome = create(&set, &report, &[0], &fake, true).unwrap();
        assert_eq!(*fake.writes.borrow(), 0);
        assert_eq!(counts(&outcome.plans[0]), (4, 0));
        assert_eq!(outcome.plans[0].issues[1].blocked_by, vec!["water-v1/model"]);
    }

    #[test]
    fn cross_plan_and_numbered_dependencies_link_to_existing_issues() {
        let next = r##"
plan: next-v1
title: Next
issues:
  - {key: export, title: Export, scope: [Sources/Export/], accept: [x], depends_on: [water-v1/model, "#3"]}
"##;
        let (set, report) = load(&[("water-v1", PLAN), ("next-v1", next)]);
        let fake = Fake::default();

        let early = create(&set, &report, &[1], &fake, false).unwrap_err().to_string();
        assert!(early.contains("create plan water-v1 first"), "{early}");
        assert_eq!(*fake.writes.borrow(), 0);

        create(&set, &report, &[0], &fake, false).unwrap();
        let outcome = create(&set, &report, &[1], &fake, false).unwrap();
        assert_eq!(outcome.plans[0].issues[0].number, Some(6));
        assert_eq!(fake.blockers.borrow()[&6], vec![2, 3]);
    }

    #[test]
    fn marker_survives_comment_closing_text() {
        let (set, report) = load(&[("water-v1", PLAN)]);
        let fake = Fake::default();
        create(&set, &report, &[0], &fake, false).unwrap();
        let body = fake.issues.borrow()[2].body.clone();
        assert_eq!(body.matches("-->").count(), 2, "{body}");
        let marker = parse_marker(&body).unwrap();
        assert_eq!(marker.key.as_deref(), Some("entry-ui"));
        assert_eq!(marker.accept, vec!["Tap --> total changes"]);
        assert_eq!(marker.depends_on, vec!["water-v1/model"]);
    }
}
