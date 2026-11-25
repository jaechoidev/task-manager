//! The scheduler: which issues are ready, in what order, and how many at
//! once, across one project or several. Each issue runs as its own
//! `factory run` process, so one that dies takes nothing else with it and
//! can be picked up again later.
//!
//! `factory queue` is the scheduler with one project. `factory schedule`
//! works on every project this machine has registered, with one set of
//! places for all of them. The simulators and the agent subscriptions are
//! already shared, through the pools. What this adds is who gets the next
//! free place.

use super::issue;
use super::util;
use crate::gh;
use crate::plan::create::parse_marker;
use crate::plan::validate::scopes_overlap;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// One project the scheduler works on.
pub struct Project {
    /// A checkout of the project repository.
    pub repo_dir: PathBuf,
    /// Only issues with this label are picked. `today` is how work is chosen.
    pub label: String,
    /// At most this many of its issues at once.
    pub max_parallel: Option<usize>,
}

pub struct Options {
    pub projects: Vec<Project>,
    /// How many issues may run at once, over all projects.
    pub parallel: usize,
    /// Finish one project's work before starting the next one's. Otherwise
    /// the next free place goes to the project with the fewest issues running.
    pub in_order: bool,
    /// Never end: keep looking for issues that became ready.
    pub watch: bool,
    /// Passed on to every run.
    pub no_merge: bool,
    /// Start a further issue only while the machine has room for it.
    pub guard: bool,
    /// One project, worded as `factory queue` always was.
    pub single: bool,
}

/// What the scheduler knows about a project when it hands out a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Load {
    /// Its issues running now. A parked issue is not running.
    pub running: usize,
    /// Its issues that could start now.
    pub startable: usize,
    pub max: Option<usize>,
}

/// Which project gets the next free place, if any.
///
/// In order: the first project with anything running or startable is the
/// one being worked on, and nobody else starts until it has neither. A
/// project with only parked issues is not in the way.
///
/// In parallel: the project with the fewest issues running, and the earlier
/// one when two have the same.
pub fn next_project(in_order: bool, loads: &[Load]) -> Option<usize> {
    let may = |load: &Load| load.startable > 0 && load.max.is_none_or(|max| load.running < max);
    if in_order {
        let current = loads.iter().position(|load| load.running > 0 || load.startable > 0)?;
        return may(&loads[current]).then_some(current);
    }
    loads.iter().enumerate().filter(|(_, load)| may(load)).min_by_key(|(index, load)| (load.running, *index)).map(|(index, _)| index)
}

/// An open issue the queue may run.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub number: u64,
    pub key: String,
    pub title: String,
    pub scope: Vec<String>,
    /// 0 runs first.
    pub priority: u8,
    /// Open issues this one is blocked by.
    pub blockers: Vec<u64>,
    /// Carries the `blocked` label: an earlier run needs a person.
    pub held: bool,
}

/// `priority:high` runs before unlabelled and `priority:normal`, which run
/// before `priority:low`.
pub fn priority_of(labels: &[String]) -> u8 {
    if labels.iter().any(|l| l == "priority:high") {
        0
    } else if labels.iter().any(|l| l == "priority:low") {
        2
    } else {
        1
    }
}

/// Issues that could start now, most urgent first.
pub fn ready(candidates: &[Candidate], tried: &BTreeSet<u64>) -> Vec<Candidate> {
    let mut found: Vec<Candidate> =
        candidates.iter().filter(|c| c.blockers.is_empty() && !c.held && !tried.contains(&c.number)).cloned().collect();
    found.sort_by_key(|c| (c.priority, c.number));
    found
}

/// Which ready issues to start, given what is running and how many more may
/// run. Two issues whose scopes overlap never run together, whatever the
/// plan said: their developers would edit the same files.
pub fn pick(ready: &[Candidate], running: &[Candidate], slots: usize) -> Vec<Candidate> {
    let mut busy: Vec<&Candidate> = running.iter().collect();
    let mut picked: Vec<Candidate> = Vec::new();
    for candidate in ready {
        if picked.len() >= slots {
            break;
        }
        if busy.iter().any(|other| scopes_overlap(&candidate.scope, &other.scope).is_some()) {
            continue;
        }
        picked.push(candidate.clone());
        busy.push(candidate);
    }
    picked
}

fn labels_of(value: &Value) -> Vec<String> {
    value
        .get("labels")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Reads the open issues that carry the label and were made by `plan create`.
fn candidates(repo: &str, label: &str) -> Result<Vec<Candidate>> {
    let listed = gh::api_list(&format!("repos/{repo}/issues?state=open&labels={}", gh::encode(label)))?;
    let mut found = Vec::new();
    for value in listed {
        if value.get("pull_request").is_some() {
            continue;
        }
        let Some(number) = value.get("number").and_then(Value::as_u64) else { continue };
        let body = value.get("body").and_then(Value::as_str).unwrap_or("");
        // A plan's parent issue has no key. It is never run.
        let Some((marker, key)) = parse_marker(body).and_then(|m| m.key.clone().map(|key| (m, key))) else { continue };
        let labels = labels_of(&value);
        found.push(Candidate {
            number,
            key,
            title: value.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
            scope: marker.scope.clone(),
            priority: priority_of(&labels),
            blockers: issue::open_blockers(repo, number)?,
            held: labels.iter().any(|l| l == "blocked"),
        });
    }
    Ok(found)
}

struct Running {
    /// Which project it belongs to, by position.
    project: usize,
    candidate: Candidate,
    child: Child,
    log: PathBuf,
    started: Instant,
}

fn last_line(log: &Path) -> String {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .rev()
        .find(|line| line.starts_with("MERGED") || line.starts_with("PASSED") || line.starts_with("FAILED") || line.starts_with("error"))
        .unwrap_or("no result line")
        .to_string()
}

/// How often the queue looks for newly ready issues while a place is free.
fn look_again() -> Duration {
    Duration::from_secs(std::env::var("FACTORY_QUEUE_LOOK_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(30))
}

fn pause() -> Duration {
    match std::env::var("FACTORY_POLL_SECONDS").ok().and_then(|v| v.parse::<u64>().ok()) {
        Some(0) => Duration::from_millis(100),
        Some(seconds) => Duration::from_secs(seconds.min(5)),
        None => Duration::from_secs(1),
    }
}

/// What the scheduler keeps per project.
struct State {
    repo_dir: PathBuf,
    /// `owner/name`.
    repo: String,
    label: String,
    max: Option<usize>,
    logs: PathBuf,
    /// The latest list of labelled issues.
    all: Vec<Candidate>,
    tried: BTreeSet<u64>,
    /// Issues whose next call nobody has quota for, and when to look again.
    parked: Vec<(Candidate, u64)>,
    merged: Vec<u64>,
    opened: Vec<u64>,
    failed: Vec<u64>,
}

/// Runs every ready issue of every project until none is left, or for ever
/// with `watch`. Returns false when an issue ended blocked.
pub fn run(opts: &Options) -> Result<bool> {
    if opts.parallel == 0 {
        bail!("--parallel must be at least 1");
    }
    if opts.projects.is_empty() {
        bail!("no project to work on. Add one with `factory project add <checkout>`");
    }
    let home = util::home()?;
    let program = std::env::current_exe().context("finding the factory program")?;
    let store = super::state::Store::open(&home)?;
    let mut states: Vec<State> = Vec::new();
    for project in &opts.projects {
        let repo_dir = project.repo_dir.canonicalize().with_context(|| format!("{} does not exist", project.repo_dir.display()))?;
        let repo = super::repo_name(&repo_dir)?;
        if states.iter().any(|known| known.repo == repo) {
            bail!("{repo} is listed twice");
        }
        let logs = home.join("queue").join(util::repo_dir_name(&repo));
        std::fs::create_dir_all(&logs)?;
        states.push(State {
            repo_dir,
            repo,
            label: project.label.clone(),
            max: project.max_parallel,
            logs,
            all: Vec::new(),
            tried: BTreeSet::new(),
            parked: Vec::new(),
            merged: Vec::new(),
            opened: Vec::new(),
            failed: Vec::new(),
        });
    }
    // How an issue is named in the output: `#7`, or `todo-trial#7` when
    // there are several projects.
    let single = opts.single && states.len() == 1;
    let short: Vec<String> = states.iter().map(|state| state.repo.rsplit('/').next().unwrap_or(&state.repo).to_string()).collect();
    let tag = |project: usize, number: u64| if single { format!("#{number}") } else { format!("{}#{number}", short[project]) };
    if single {
        println!("queue {}: issues labelled '{}', up to {} at once", states[0].repo, states[0].label, opts.parallel);
    } else {
        let how = if opts.in_order { "one project after the other" } else { "projects side by side" };
        let watch = if opts.watch { ", and it keeps looking for new work until stopped" } else { "" };
        println!("schedule: {} project(s), up to {} issue(s) at once, {how}{watch}", states.len(), opts.parallel);
        for state in &states {
            let max = state.max.map(|max| format!(", at most {max} at once")).unwrap_or_default();
            println!("  {}: issues labelled '{}'{max}", state.repo, state.label);
        }
    }

    let mut running: Vec<Running> = Vec::new();
    let mut held_back: Option<String> = None;
    let mut ended_since_handoff = false;
    loop {
        for state in states.iter_mut() {
            state.all = candidates(&state.repo, &state.label)?;
        }
        // A parked issue whose time has come goes first: it is half done.
        let now = util::now_secs();
        let mut starts: Vec<(usize, Candidate)> = Vec::new();
        for (project, state) in states.iter_mut().enumerate() {
            let mut still = Vec::new();
            for (candidate, wake) in state.parked.drain(..) {
                if wake <= now && running.len() + starts.len() < opts.parallel {
                    starts.push((project, candidate));
                } else {
                    still.push((candidate, wake));
                }
            }
            state.parked = still;
        }
        // Then the free places, one at a time: who gets it, and which of
        // that project's issues.
        while running.len() + starts.len() < opts.parallel {
            let mut loads = Vec::new();
            let mut first: Vec<Option<Candidate>> = Vec::new();
            for (project, state) in states.iter().enumerate() {
                let starting: Vec<&Candidate> = starts.iter().filter(|(p, _)| *p == project).map(|(_, c)| c).collect();
                let mine: Vec<&Candidate> = running.iter().filter(|r| r.project == project).map(|r| &r.candidate).collect();
                // A parked issue keeps its files, so nothing that overlaps it may start.
                let busy: Vec<Candidate> = mine.iter().chain(starting.iter()).map(|c| (*c).clone()).chain(state.parked.iter().map(|(c, _)| c.clone())).collect();
                let waiting: Vec<Candidate> = ready(&state.all, &state.tried).into_iter().filter(|c| !starting.iter().any(|s| s.number == c.number)).collect();
                let startable = pick(&waiting, &busy, usize::MAX);
                loads.push(Load { running: mine.len() + starting.len(), startable: startable.len(), max: state.max });
                first.push(startable.into_iter().next());
            }
            let Some(project) = next_project(opts.in_order, &loads) else { break };
            let Some(candidate) = first[project].take() else { break };
            // A busy machine gets no further issue. The first one always starts.
            if opts.guard && running.len() + starts.len() > 0 {
                let room = super::capacity::room();
                if room.simulator_steps == 0 {
                    if held_back.as_deref() != Some(room.reason.as_str()) {
                        println!("  holding back {} {}: {}", tag(project, candidate.number), candidate.key, room.reason);
                    }
                    held_back = Some(room.reason);
                    break;
                }
            }
            held_back = None;
            starts.push((project, candidate));
        }
        for (project, candidate) in starts {
            let state = &mut states[project];
            let log = state.logs.join(format!("issue-{}-{}.log", candidate.number, util::run_id()));
            let out = std::fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
            let err = out.try_clone()?;
            let mut command = Command::new(&program);
            command.args(["run", &candidate.number.to_string(), "--repo-dir"]).arg(&state.repo_dir);
            if opts.no_merge {
                command.arg("--no-merge");
            }
            let child = command.stdin(Stdio::null()).stdout(out).stderr(err).spawn().context("starting a run")?;
            println!("  {} {} started. Log: {}", tag(project, candidate.number), candidate.key, log.display());
            state.tried.insert(candidate.number);
            running.push(Running { project, candidate, child, log, started: Instant::now() });
        }

        if running.is_empty() {
            let wake = states.iter().flat_map(|state| state.parked.iter().map(|(_, wake)| *wake)).min();
            if let Some(wake) = wake {
                // Only parked issues are left. Nothing is held while waiting.
                let list: Vec<String> = states.iter().enumerate().flat_map(|(project, state)| state.parked.iter().map(move |(c, _)| (project, c.number))).map(|(project, number)| tag(project, number)).collect();
                println!("  waiting until {} for quota: {} parked", util::clock(wake), list.join(", "));
                let wait = Duration::from_secs(wake.saturating_sub(util::now_secs()).max(1));
                // With `watch`, new work may arrive before that.
                std::thread::sleep(if opts.watch { wait.min(look_again()) } else { wait });
                continue;
            }
            if opts.watch {
                if ended_since_handoff {
                    write_handoff();
                    ended_since_handoff = false;
                }
                std::thread::sleep(look_again());
                continue;
            }
            // Nothing runs and nothing can start. Say why for what is left.
            for (project, state) in states.iter().enumerate() {
                for candidate in state.all.iter().filter(|c| !state.tried.contains(&c.number)) {
                    let why = if candidate.held {
                        "carries the 'blocked' label, so a person decides first".to_string()
                    } else {
                        let blockers: Vec<String> = candidate.blockers.iter().map(|n| format!("#{n}")).collect();
                        format!("is blocked by {}, still open", blockers.join(", "))
                    };
                    println!("  {} {} not run: it {why}", tag(project, candidate.number), candidate.key);
                }
            }
            break;
        }

        // Wait for one run to end. With a place free, also look again now
        // and then: an issue may have become ready without a run ending,
        // because a person labelled it, or because GitHub's list was a few
        // seconds behind when the queue started.
        let looked = Instant::now();
        let index = loop {
            let mut ended = None;
            for (index, run) in running.iter_mut().enumerate() {
                if run.child.try_wait()?.is_some() {
                    ended = Some(index);
                    break;
                }
            }
            match ended {
                Some(index) => break Some(index),
                None if running.len() < opts.parallel && looked.elapsed() >= look_again() => break None,
                None => std::thread::sleep(Duration::from_millis(200)),
            }
        };
        let Some(index) = index else { continue };
        let mut done = running.remove(index);
        let status = done.child.wait()?;
        let result = last_line(&done.log);
        let number = done.candidate.number;
        let name = tag(done.project, number);
        let state = &mut states[done.project];
        if status.code() == Some(i32::from(super::PARKED_EXIT_CODE)) {
            // Out of quota, not failed. It gives its place to other work and
            // comes back at the step it stopped at.
            let wake = store.issue(&state.repo, number)?.and_then(|row| row.wake_at).unwrap_or(util::now_secs() + 60);
            println!("  {name} {} parked after {:.0} s, until {}: {result}", done.candidate.key, done.started.elapsed().as_secs_f64(), util::clock(wake));
            state.parked.push((done.candidate, wake));
            continue;
        }
        if !status.success() && result.contains(super::ALREADY_RUNNING) {
            // Someone started it by hand, or another scheduler has it. That
            // is not a failure of the issue, and nobody needs to look.
            println!("  {name} {} left alone: another process is running it", done.candidate.key);
            continue;
        }
        println!("  {name} {} ended after {:.0} s: {result}", done.candidate.key, done.started.elapsed().as_secs_f64());
        ended_since_handoff = true;
        if !status.success() {
            state.failed.push(number);
        } else if result.starts_with("MERGED") {
            state.merged.push(number);
            // GitHub closes the issue a moment after the merge. Issues it
            // blocks only become ready once it is closed.
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                let issue = gh::api("GET", &format!("repos/{}/issues/{number}", state.repo), None)?;
                if issue.get("state").and_then(Value::as_str) == Some("closed") {
                    break;
                }
                std::thread::sleep(pause());
            }
        } else {
            state.opened.push(number);
        }
    }

    let list = |numbers: &[u64]| if numbers.is_empty() { "none".to_string() } else { numbers.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ") };
    for state in &states {
        let who = if single { "queue done.".to_string() } else { format!("schedule done for {}.", state.repo) };
        println!("{who} Merged: {}. Pull request open: {}. Blocked: {}", list(&state.merged), list(&state.opened), list(&state.failed));
    }
    write_handoff();
    Ok(states.iter().all(|state| state.failed.is_empty()))
}

/// The day's handoff, written again with what just happened.
fn write_handoff() {
    match super::handoff::export(&super::handoff::HandoffOptions { date: None, out: None }) {
        Ok(files) => files.iter().for_each(|file| println!("handoff: {}", file.display())),
        Err(e) => eprintln!("warning: could not write the handoff: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(number: u64, scope: &[&str], priority: u8) -> Candidate {
        Candidate {
            number,
            key: format!("k{number}"),
            title: String::new(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            priority,
            blockers: Vec::new(),
            held: false,
        }
    }

    fn numbers(list: &[Candidate]) -> Vec<u64> {
        list.iter().map(|c| c.number).collect()
    }

    #[test]
    fn side_by_side_the_next_place_goes_to_the_project_with_the_fewest_running() {
        let load = |running, startable| Load { running, startable, max: None };
        // Nobody runs yet: the first project in the list.
        assert_eq!(next_project(false, &[load(0, 2), load(0, 2)]), Some(0));
        // Then the other one, and so on in turn.
        assert_eq!(next_project(false, &[load(1, 1), load(0, 2)]), Some(1));
        assert_eq!(next_project(false, &[load(1, 1), load(1, 1)]), Some(0));
        // A project with nothing to start is passed over, however little it runs.
        assert_eq!(next_project(false, &[load(0, 0), load(3, 1)]), Some(1));
        assert_eq!(next_project(false, &[load(0, 0), load(2, 0)]), None);
        // A project at its own limit is passed over too.
        let capped = Load { running: 1, startable: 4, max: Some(1) };
        assert_eq!(next_project(false, &[capped, load(5, 1)]), Some(1));
        assert_eq!(next_project(false, &[capped]), None);
    }

    #[test]
    fn in_order_nobody_starts_until_the_project_ahead_has_nothing_left() {
        let load = |running, startable| Load { running, startable, max: None };
        assert_eq!(next_project(true, &[load(0, 2), load(0, 2)]), Some(0));
        assert_eq!(next_project(true, &[load(1, 1), load(0, 2)]), Some(0));
        // The first still has an issue running, and what is left of it waits
        // on that one. The second does not jump in.
        assert_eq!(next_project(true, &[load(1, 0), load(0, 2)]), None);
        // Nothing running and nothing startable, parked issues or not: next.
        assert_eq!(next_project(true, &[load(0, 0), load(0, 2)]), Some(1));
        assert_eq!(next_project(true, &[load(0, 0), load(0, 0)]), None);
        // Its own limit holds the first back without letting the second in.
        assert_eq!(next_project(true, &[Load { running: 1, startable: 2, max: Some(1) }, load(0, 2)]), None);
    }

    #[test]
    fn priority_comes_from_labels() {
        let labels = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(priority_of(&labels(&["today", "priority:high"])), 0);
        assert_eq!(priority_of(&labels(&["today"])), 1);
        assert_eq!(priority_of(&labels(&["priority:normal"])), 1);
        assert_eq!(priority_of(&labels(&["priority:low", "size:S"])), 2);
    }

    #[test]
    fn ready_issues_are_ordered_by_priority_then_number() {
        let mut blocked = candidate(4, &["d/"], 0);
        blocked.blockers = vec![9];
        let mut held = candidate(5, &["e/"], 0);
        held.held = true;
        let all = vec![candidate(8, &["a/"], 1), candidate(3, &["b/"], 2), candidate(7, &["c/"], 0), blocked, held, candidate(6, &["f/"], 1)];
        assert_eq!(numbers(&ready(&all, &BTreeSet::new())), vec![7, 6, 8, 3]);
        // An issue this queue already ran is not started again.
        assert_eq!(numbers(&ready(&all, &BTreeSet::from([7, 6]))), vec![8, 3]);
    }

    #[test]
    fn overlapping_scopes_never_run_together() {
        let list = vec![candidate(1, &["Sources/UI/List/"], 0), candidate(2, &["Sources/UI/"], 0), candidate(3, &["Sources/Model/"], 1)];
        // #2 contains #1's path, so it waits. #3 is elsewhere.
        assert_eq!(numbers(&pick(&list, &[], 3)), vec![1, 3]);
        // With #2 running, #1 has to wait and #3 may start.
        let running = vec![candidate(2, &["Sources/UI/"], 0)];
        assert_eq!(numbers(&pick(&[list[0].clone(), list[2].clone()], &running, 2)), vec![3]);
        // Sibling folders with a common prefix in their names don't overlap.
        let siblings = vec![candidate(1, &["Sources/UI/List/"], 0), candidate(2, &["Sources/UI/ListDetail/"], 0)];
        assert_eq!(numbers(&pick(&siblings, &[], 2)), vec![1, 2]);
    }

    #[test]
    fn only_as_many_start_as_there_are_slots() {
        let list = vec![candidate(1, &["a/"], 0), candidate(2, &["b/"], 0), candidate(3, &["c/"], 0)];
        assert_eq!(numbers(&pick(&list, &[], 2)), vec![1, 2]);
        assert_eq!(numbers(&pick(&list, &[], 0)), Vec::<u64>::new());
        // A scope that can't be read is treated as overlapping everything.
        let odd = vec![candidate(1, &["a/"], 0), candidate(2, &["/absolute"], 0)];
        assert_eq!(numbers(&pick(&odd, &[], 2)), vec![1]);
    }
}
