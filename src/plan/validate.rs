//! Decides whether a set of plan files is acceptable. No LLM, no network.
//!
//! Checks: required fields, unique keys, references, cycles, unordered scope
//! overlap, flow files and labels. Then it computes the parallel batches.

use super::model::{is_key, is_plan_id, Dep, PlanSet};
use crate::labels;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

pub struct Options {
    /// Where existing Maestro flows live, relative to the working directory.
    pub flows_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diag {
    pub subject: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IssueNode {
    pub key: String,
    pub title: String,
    /// Keys in the same plan this issue depends on.
    pub depends_on: Vec<String>,
    /// Dependencies outside the plan: `other-plan/key` or `#42`.
    pub external: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanReport {
    pub path: String,
    pub id: String,
    pub title: String,
    pub issue_count: usize,
    pub errors: Vec<Diag>,
    pub warnings: Vec<Diag>,
    /// Issues that may run at the same time, in order. Empty when the
    /// dependency graph could not be built.
    pub batches: Vec<Vec<String>>,
    pub nodes: Vec<IssueNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub ok: bool,
    pub file_errors: Vec<Diag>,
    pub plans: Vec<PlanReport>,
}

impl Report {
    pub fn error_count(&self) -> usize {
        self.file_errors.len() + self.plans.iter().map(|p| p.errors.len()).sum::<usize>()
    }
}

struct Node {
    plan: usize,
    issue: usize,
    key: String,
}

/// A scope path split into components, plus the text as written.
struct ScopePath {
    parts: Vec<String>,
    shown: String,
}

fn parse_scope(raw: &str) -> Result<ScopePath, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("scope entry is empty".into());
    }
    if text.starts_with('/') {
        return Err(format!("scope '{text}' must be relative to the repo root"));
    }
    if text.contains(['*', '?', '[']) {
        return Err(format!("scope '{text}' must be a path, not a pattern"));
    }
    let trimmed = text.trim_start_matches("./").trim_end_matches('/');
    let parts: Vec<String> = trimmed.split('/').map(str::to_string).collect();
    if trimmed.is_empty() || parts.iter().any(|p| p.is_empty() || p == "." || p == "..") {
        return Err(format!("scope '{text}' must name a path inside the repo"));
    }
    Ok(ScopePath { parts, shown: text.to_string() })
}

/// Returns the narrower of two paths when one contains the other.
fn overlap<'a>(a: &'a ScopePath, b: &'a ScopePath) -> Option<&'a str> {
    let n = a.parts.len().min(b.parts.len());
    if a.parts[..n] != b.parts[..n] {
        return None;
    }
    Some(if a.parts.len() >= b.parts.len() { &a.shown } else { &b.shown })
}

fn valid_flow_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && !n.starts_with('/')
        && !n.split('/').any(|p| p.is_empty() || p == "..")
        && (n.ends_with(".yaml") || n.ends_with(".yml"))
}

fn valid_accessibility_id(id: &str) -> bool {
    !id.is_empty() && !id.chars().any(char::is_whitespace)
}

pub fn validate(set: &PlanSet, opts: &Options) -> Report {
    let file_errors: Vec<Diag> = set
        .file_errors
        .iter()
        .map(|(path, msg)| Diag { subject: path.display().to_string(), message: msg.clone() })
        .collect();

    let mut reports: Vec<PlanReport> = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut by_key: HashMap<(usize, String), usize> = HashMap::new();
    let mut plan_by_id: HashMap<String, usize> = HashMap::new();
    let mut scopes: HashMap<usize, Vec<ScopePath>> = HashMap::new();

    // Pass 1: fields that can be checked one plan at a time.
    for (pi, loaded) in set.plans.iter().enumerate() {
        let file = &loaded.file;
        let path = loaded.path.display().to_string();
        let id = file.plan.clone().unwrap_or_default().trim().to_string();
        let label = if id.is_empty() { path.clone() } else { id.clone() };
        let mut errors = Vec::new();
        let mut err = |subject: &str, message: String| {
            errors.push(Diag { subject: subject.to_string(), message });
        };

        if id.is_empty() {
            err(&label, "missing 'plan' (the plan ID)".into());
        } else if !is_plan_id(&id) {
            err(&label, format!("plan ID '{id}' may only use a-z, 0-9, '-', '_' and '.'"));
        } else {
            let stem = loaded.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem != id {
                err(&label, format!("file must be named {id}.yaml to match its plan ID"));
            }
            match plan_by_id.get(&id) {
                Some(&other) => err(
                    &label,
                    format!("plan ID is already used by {}", set.plans[other].path.display()),
                ),
                None => {
                    plan_by_id.insert(id.clone(), pi);
                }
            }
        }
        if file.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
            err(&label, "missing 'title' (the parent issue title)".into());
        }
        if file.issues.is_empty() {
            err(&label, "plan has no issues".into());
        }

        let mut seen: HashMap<String, usize> = HashMap::new();
        for (ii, issue) in file.issues.iter().enumerate() {
            let key = issue.key.clone().unwrap_or_default().trim().to_string();
            let subject = if key.is_empty() {
                format!("{label}/(issue {})", ii + 1)
            } else {
                format!("{label}/{key}")
            };

            let mut is_node = false;
            if key.is_empty() {
                err(&subject, "missing 'key'".into());
            } else if !is_key(&key) {
                err(&subject, format!("key '{key}' may only use a-z, 0-9, '-' and '_'"));
            } else if let Some(first) = seen.get(&key) {
                err(&subject, format!("key '{key}' is defined twice (issues {} and {})", first + 1, ii + 1));
            } else {
                seen.insert(key.clone(), ii);
                is_node = true;
            }

            if issue.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
                err(&subject, "missing 'title'".into());
            }
            if issue.accept.is_empty() {
                err(&subject, "missing 'accept' (acceptance criteria)".into());
            } else if issue.accept.iter().any(|a| a.trim().is_empty()) {
                err(&subject, "an acceptance criterion is empty".into());
            }

            let mut parsed_scopes = Vec::new();
            if issue.scope.is_empty() {
                err(&subject, "missing 'scope' (the paths this issue may change)".into());
            }
            for raw in &issue.scope {
                match parse_scope(raw) {
                    Ok(s) => parsed_scopes.push(s),
                    Err(message) => err(&subject, message),
                }
            }

            let mut sizes = 0;
            for name in &issue.labels {
                if labels::is_state_label(name) {
                    err(&subject, format!("label '{name}' tracks state and can't be set in a plan"));
                } else if !labels::is_plan_label(name) {
                    err(
                        &subject,
                        format!("unknown label '{name}' (allowed: {})", labels::plan_label_names().join(", ")),
                    );
                }
                if name.starts_with("size:") {
                    sizes += 1;
                }
            }
            if sizes > 1 {
                err(&subject, "more than one size label".into());
            }
            for id in &issue.accessibility_ids {
                if !valid_accessibility_id(id) {
                    err(&subject, format!("accessibility ID '{id}' must be non-empty with no spaces"));
                }
            }
            for flow in &issue.flows {
                if !valid_flow_name(flow.file()) {
                    err(&subject, format!("flow '{}' must be a .yaml file name", flow.file()));
                }
            }

            if is_node {
                let ni = nodes.len();
                nodes.push(Node { plan: pi, issue: ii, key: key.clone() });
                by_key.insert((pi, key), ni);
                scopes.insert(ni, parsed_scopes);
            }
        }

        reports.push(PlanReport {
            path,
            id: label,
            title: file.title.clone().unwrap_or_default().trim().to_string(),
            issue_count: file.issues.len(),
            errors,
            warnings: Vec::new(),
            batches: Vec::new(),
            nodes: Vec::new(),
        });
    }

    let subject_of = |ni: usize| format!("{}/{}", reports[nodes[ni].plan].id, nodes[ni].key);
    let subjects: Vec<String> = (0..nodes.len()).map(subject_of).collect();

    // Pass 2: resolve depends_on into graph edges.
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut external: Vec<Vec<String>> = vec![Vec::new(); nodes.len()];
    for ni in 0..nodes.len() {
        let pi = nodes[ni].plan;
        let issue = &set.plans[pi].file.issues[nodes[ni].issue];
        let mut push_err = |message: String| {
            reports[pi].errors.push(Diag { subject: subjects[ni].clone(), message });
        };
        for raw in &issue.depends_on {
            let shown = raw.display();
            let target = match raw.parse() {
                Err(message) => {
                    push_err(message);
                    continue;
                }
                Ok(Dep::Issue(n)) => {
                    external[ni].push(format!("#{n}"));
                    continue;
                }
                Ok(Dep::Local(key)) => (pi, key, false),
                Ok(Dep::Cross { plan, key }) => match plan_by_id.get(&plan) {
                    Some(&other) => (other, key, other != pi),
                    None => {
                        push_err(format!(
                            "depends_on '{shown}': plan '{plan}' is not in the plans directory. A plan must be merged before another plan refers to it"
                        ));
                        continue;
                    }
                },
            };
            let (target_plan, key, cross) = target;
            match by_key.get(&(target_plan, key)) {
                None => push_err(format!("depends_on '{shown}' does not exist")),
                Some(&t) if t == ni => push_err(format!("depends_on '{shown}' points at itself")),
                Some(&t) => {
                    if deps[ni].contains(&t) {
                        continue;
                    }
                    deps[ni].push(t);
                    if cross {
                        external[ni].push(subjects[t].clone());
                    }
                }
            }
        }
    }

    // Pass 3: cycles.
    let cycles = find_cycles(&deps);
    let mut plans_with_cycle: BTreeSet<usize> = BTreeSet::new();
    for cycle in &cycles {
        let involved: BTreeSet<usize> = cycle.iter().map(|&n| nodes[n].plan).collect();
        let single_plan = involved.len() == 1;
        let name = |n: usize| if single_plan { nodes[n].key.clone() } else { subjects[n].clone() };
        let mut path: Vec<String> = cycle.iter().map(|&n| name(n)).collect();
        path.push(name(cycle[0]));
        for &pi in &involved {
            plans_with_cycle.insert(pi);
            let id = reports[pi].id.clone();
            reports[pi].errors.push(Diag {
                subject: id,
                message: format!("dependency cycle: {}. None of these can ever start", path.join(" → ")),
            });
        }
    }

    // Pass 4: every issue an issue depends on, directly or not.
    let ancestors: Vec<BTreeSet<usize>> = (0..nodes.len()).map(|n| reachable(n, &deps)).collect();
    let ordered = |a: usize, b: usize| ancestors[a].contains(&b) || ancestors[b].contains(&a);

    // Pass 5: two issues with no order between them must not share paths.
    let mut by_plan: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (ni, node) in nodes.iter().enumerate() {
        by_plan.entry(node.plan).or_default().push(ni);
    }
    for (&pi, members) in &by_plan {
        for (i, &a) in members.iter().enumerate() {
            for &b in &members[i + 1..] {
                if ordered(a, b) {
                    continue;
                }
                let mut shared: Vec<&str> = Vec::new();
                for sa in &scopes[&a] {
                    for sb in &scopes[&b] {
                        if let Some(path) = overlap(sa, sb) {
                            if !shared.contains(&path) {
                                shared.push(path);
                            }
                        }
                    }
                }
                if !shared.is_empty() {
                    let id = reports[pi].id.clone();
                    reports[pi].errors.push(Diag {
                        subject: id,
                        message: format!(
                            "{} and {} overlap on {} with no order; add depends_on",
                            nodes[a].key,
                            nodes[b].key,
                            shared.join(", ")
                        ),
                    });
                }
            }
        }
    }

    // Pass 6: flows. A flow must exist already, or be created by this issue
    // or by one it depends on.
    let mut creators: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (ni, node) in nodes.iter().enumerate() {
        for flow in &set.plans[node.plan].file.issues[node.issue].flows {
            if flow.is_new() && valid_flow_name(flow.file()) {
                creators.entry(flow.file().trim().to_string()).or_default().push(ni);
            }
        }
    }
    for (name, made_by) in &creators {
        if made_by.len() > 1 {
            let who: Vec<&str> = made_by.iter().map(|&n| subjects[n].as_str()).collect();
            for &pi in &made_by.iter().map(|&n| nodes[n].plan).collect::<BTreeSet<_>>() {
                let id = reports[pi].id.clone();
                reports[pi].errors.push(Diag {
                    subject: id,
                    message: format!("flow '{name}' is marked new by more than one issue: {}", who.join(", ")),
                });
            }
        }
    }
    for (ni, node) in nodes.iter().enumerate() {
        let pi = node.plan;
        for flow in &set.plans[pi].file.issues[node.issue].flows {
            let name = flow.file().trim();
            if !valid_flow_name(name) {
                continue;
            }
            let on_disk = opts.flows_dir.join(name).is_file();
            if flow.is_new() {
                if on_disk {
                    reports[pi].warnings.push(Diag {
                        subject: subjects[ni].clone(),
                        message: format!("flow '{name}' is marked new but already exists"),
                    });
                }
                continue;
            }
            if on_disk {
                continue;
            }
            let message = match creators.get(name).and_then(|c| c.first()) {
                Some(&creator) if ancestors[ni].contains(&creator) => continue,
                Some(&creator) => format!(
                    "flow '{name}' is created by {}; add it to depends_on",
                    subjects[creator]
                ),
                None => format!(
                    "flow '{name}' does not exist in {} and is not marked new",
                    opts.flows_dir.display()
                ),
            };
            reports[pi].errors.push(Diag { subject: subjects[ni].clone(), message });
        }
    }

    // Pass 7: node list and batches for the report.
    for (&pi, members) in &by_plan {
        let mut level: HashMap<usize, usize> = HashMap::new();
        let no_cycle = !plans_with_cycle.contains(&pi);
        for &ni in members {
            let local: Vec<usize> = deps[ni].iter().copied().filter(|&d| nodes[d].plan == pi).collect();
            let issue = &set.plans[pi].file.issues[nodes[ni].issue];
            reports[pi].nodes.push(IssueNode {
                key: nodes[ni].key.clone(),
                title: issue.title.clone().unwrap_or_default().trim().to_string(),
                depends_on: local.iter().map(|&d| nodes[d].key.clone()).collect(),
                external: external[ni].clone(),
            });
            if no_cycle {
                let depth = ancestors[ni].iter().filter(|&&a| nodes[a].plan == pi).count();
                level.insert(ni, depth);
            }
        }
        if no_cycle {
            // An issue's batch is one past the latest batch it depends on.
            let mut order: Vec<usize> = members.clone();
            order.sort_by_key(|n| level[n]);
            let mut batch_of: HashMap<usize, usize> = HashMap::new();
            for &ni in &order {
                let b = deps[ni]
                    .iter()
                    .filter(|&&d| nodes[d].plan == pi)
                    .map(|d| batch_of[d] + 1)
                    .max()
                    .unwrap_or(0);
                batch_of.insert(ni, b);
            }
            let count = batch_of.values().copied().max().map_or(0, |m| m + 1);
            let mut batches = vec![Vec::new(); count];
            for &ni in members {
                batches[batch_of[&ni]].push(nodes[ni].key.clone());
            }
            reports[pi].batches = batches;
        }
    }

    let ok = file_errors.is_empty() && reports.iter().all(|r| r.errors.is_empty());
    Report { ok, file_errors, plans: reports }
}

/// Each cycle is returned once, as the list of nodes on it.
fn find_cycles(deps: &[Vec<usize>]) -> Vec<Vec<usize>> {
    fn visit(
        u: usize,
        deps: &[Vec<usize>],
        color: &mut [u8],
        stack: &mut Vec<usize>,
        seen: &mut BTreeSet<Vec<usize>>,
        out: &mut Vec<Vec<usize>>,
    ) {
        color[u] = 1;
        stack.push(u);
        for &v in &deps[u] {
            if color[v] == 0 {
                visit(v, deps, color, stack, seen, out);
            } else if color[v] == 1 {
                let start = stack.iter().position(|&x| x == v).unwrap_or(0);
                let cycle = stack[start..].to_vec();
                let mut canon = cycle.clone();
                canon.sort_unstable();
                if seen.insert(canon) {
                    out.push(cycle);
                }
            }
        }
        stack.pop();
        color[u] = 2;
    }

    let mut color = vec![0u8; deps.len()];
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for start in 0..deps.len() {
        if color[start] == 0 {
            visit(start, deps, &mut color, &mut Vec::new(), &mut seen, &mut out);
        }
    }
    out
}

fn reachable(from: usize, deps: &[Vec<usize>]) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut todo: Vec<usize> = deps[from].clone();
    while let Some(n) = todo.pop() {
        if n != from && seen.insert(n) {
            todo.extend(deps[n].iter().copied());
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::model::{parse_str, LoadedPlan};

    fn set(files: &[(&str, &str)]) -> PlanSet {
        let mut set = PlanSet::default();
        for (name, text) in files {
            match parse_str(text) {
                Ok(file) => set.plans.push(LoadedPlan { path: PathBuf::from(format!("plans/{name}.yaml")), file }),
                Err(e) => set.file_errors.push((PathBuf::from(*name), e)),
            }
        }
        set
    }

    fn check(files: &[(&str, &str)]) -> Report {
        validate(&set(files), &Options { flows_dir: PathBuf::from("/nonexistent") })
    }

    fn messages(report: &Report) -> Vec<String> {
        report
            .plans
            .iter()
            .flat_map(|p| p.errors.iter().map(|d| format!("{}: {}", d.subject, d.message)))
            .collect()
    }

    const WATER: &str = r#"
plan: water-v1
title: Water tracker
issues:
  - key: model
    title: Water intake model
    scope: [Sources/Model/]
    accept: ["Unit tests cover add, undo, daily total"]
  - key: entry-ui
    title: Add-water screen
    scope: [Sources/UI/Entry/]
    accept: ["Adding water updates the total"]
    depends_on: [model]
    flows: [{file: add_water.yaml, new: true}]
  - key: history-ui
    title: History screen
    scope: [Sources/UI/History/]
    accept: ["History lists each day"]
    depends_on: [model]
"#;

    #[test]
    fn valid_plan_gets_parallel_batches() {
        let report = check(&[("water-v1", WATER)]);
        assert!(report.ok, "{:?}", messages(&report));
        assert_eq!(report.plans[0].batches, vec![vec!["model"], vec!["entry-ui", "history-ui"]]);
    }

    #[test]
    fn typo_in_depends_on_is_reported() {
        let text = WATER.replace("depends_on: [model]\n    flows", "depends_on: [modle]\n    flows");
        let report = check(&[("water-v1", &text)]);
        assert!(!report.ok);
        assert!(messages(&report).contains(&"water-v1/entry-ui: depends_on 'modle' does not exist".to_string()));
    }

    #[test]
    fn cycle_is_reported_once() {
        let text = r#"
plan: loop-v1
title: Loop
issues:
  - {key: a, title: A, scope: [a/], accept: [x], depends_on: [b]}
  - {key: b, title: B, scope: [b/], accept: [x], depends_on: [a]}
"#;
        let report = check(&[("loop-v1", text)]);
        let found = messages(&report);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("dependency cycle: a → b → a"), "{found:?}");
        assert!(report.plans[0].batches.is_empty());
    }

    #[test]
    fn unordered_scope_overlap_is_reported() {
        let text = r#"
plan: ui-v1
title: UI
issues:
  - {key: entry-ui, title: A, scope: [Sources/UI/Entry/, Sources/UI/Shared/], accept: [x]}
  - {key: settings-ui, title: B, scope: [Sources/UI/Shared/Theme.swift], accept: [x]}
"#;
        let report = check(&[("ui-v1", text)]);
        assert_eq!(
            messages(&report),
            vec!["ui-v1: entry-ui and settings-ui overlap on Sources/UI/Shared/Theme.swift with no order; add depends_on"]
        );
    }

    #[test]
    fn ordered_scope_overlap_is_allowed() {
        let text = r#"
plan: ui-v1
title: UI
issues:
  - {key: a, title: A, scope: [Sources/UI/], accept: [x]}
  - {key: b, title: B, scope: [Sources/], accept: [x], depends_on: [a]}
  - {key: c, title: C, scope: [Sources/UI/Row/], accept: [x], depends_on: [b]}
"#;
        let report = check(&[("ui-v1", text)]);
        assert!(report.ok, "{:?}", messages(&report));
        assert_eq!(report.plans[0].batches, vec![vec!["a"], vec!["b"], vec!["c"]]);
    }

    #[test]
    fn sibling_paths_do_not_overlap() {
        let a = parse_scope("Sources/UI/List/").unwrap();
        let b = parse_scope("Sources/UI/ListDetail/").unwrap();
        assert!(overlap(&a, &b).is_none());
    }

    #[test]
    fn missing_fields_are_all_reported() {
        let text = r#"
plan: thin-v1
title: Thin
issues:
  - key: a
  - {key: a, title: Dup, scope: [x/], accept: [y]}
"#;
        let found = messages(&check(&[("thin-v1", text)]));
        assert!(found.contains(&"thin-v1/a: missing 'title'".to_string()), "{found:?}");
        assert!(found.iter().any(|m| m.contains("missing 'accept'")), "{found:?}");
        assert!(found.iter().any(|m| m.contains("missing 'scope'")), "{found:?}");
        assert!(found.iter().any(|m| m.contains("defined twice")), "{found:?}");
    }

    #[test]
    fn flow_must_exist_or_be_new_or_come_from_a_dependency() {
        let text = r#"
plan: flow-v1
title: Flows
issues:
  - {key: a, title: A, scope: [a/], accept: [x], flows: [{file: a.yaml, new: true}]}
  - {key: b, title: B, scope: [b/], accept: [x], depends_on: [a], flows: [a.yaml]}
  - {key: c, title: C, scope: [c/], accept: [x], flows: [a.yaml, ghost.yaml]}
"#;
        let found = messages(&check(&[("flow-v1", text)]));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("flow 'a.yaml' is created by flow-v1/a; add it to depends_on"));
        assert!(found[1].contains("flow 'ghost.yaml' does not exist"));
    }

    #[test]
    fn cross_plan_reference_needs_a_loaded_plan() {
        let other = r##"
plan: next-v1
title: Next
issues:
  - {key: export, title: Export, scope: [Sources/Export/], accept: [x], depends_on: [water-v1/model, "#42"]}
  - {key: share, title: Share, scope: [Sources/Share/], accept: [x], depends_on: [gone-v1/thing]}
"##;
        let report = check(&[("water-v1", WATER), ("next-v1", other)]);
        let found = messages(&report);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("plan 'gone-v1' is not in the plans directory"));
        let export = &report.plans[1].nodes[0];
        assert_eq!(export.external, vec!["water-v1/model", "#42"]);
    }

    #[test]
    fn labels_and_file_name_are_checked() {
        let text = r#"
plan: label-v1
title: Labels
issues:
  - {key: a, title: A, scope: [a/], accept: [x], labels: ["size:S", "size:M", today, nope]}
"#;
        let mut plans = set(&[("label-v1", text)]);
        plans.plans[0].path = PathBuf::from("plans/other.yaml");
        let report = validate(&plans, &Options { flows_dir: PathBuf::from("/nonexistent") });
        let found = messages(&report);
        assert!(found.iter().any(|m| m.contains("file must be named label-v1.yaml")), "{found:?}");
        assert!(found.iter().any(|m| m.contains("more than one size label")), "{found:?}");
        assert!(found.iter().any(|m| m.contains("label 'today' tracks state")), "{found:?}");
        assert!(found.iter().any(|m| m.contains("unknown label 'nope'")), "{found:?}");
    }

    #[test]
    fn unknown_field_fails_the_file() {
        let text = "plan: x\ntitle: X\nissues:\n  - {key: a, title: A, scope: [a/], accept: [x], depend_on: [b]}\n";
        let report = check(&[("x", text)]);
        assert!(!report.ok);
        assert_eq!(report.file_errors.len(), 1);
        assert!(report.file_errors[0].message.contains("depend_on"));
    }
}
