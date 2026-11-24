//! The issue queue: which issues are ready, in what order, and how many at
//! once. Each issue runs as its own `factory run` process, so one that dies
//! takes nothing else with it and can be picked up again later.

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

pub struct QueueOptions {
    /// A checkout of the project repository.
    pub repo_dir: PathBuf,
    /// Only issues with this label are picked. `today` is how work is chosen.
    pub label: String,
    /// How many issues may run at once.
    pub parallel: usize,
    /// Passed on to every run.
    pub no_merge: bool,
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

fn pause() -> Duration {
    match std::env::var("FACTORY_POLL_SECONDS").ok().and_then(|v| v.parse::<u64>().ok()) {
        Some(0) => Duration::from_millis(100),
        Some(seconds) => Duration::from_secs(seconds.min(5)),
        None => Duration::from_secs(1),
    }
}

/// Runs every ready issue until none is left. Returns false when an issue
/// ended blocked.
pub fn run(opts: &QueueOptions) -> Result<bool> {
    if opts.parallel == 0 {
        bail!("--parallel must be at least 1");
    }
    let repo_dir = opts.repo_dir.canonicalize().with_context(|| format!("{} does not exist", opts.repo_dir.display()))?;
    let repo = super::repo_name(&repo_dir)?;
    let home = util::home()?;
    let logs = home.join("queue").join(util::repo_dir_name(&repo));
    std::fs::create_dir_all(&logs)?;
    let program = std::env::current_exe().context("finding the factory program")?;
    println!("queue {repo}: issues labelled '{}', up to {} at once", opts.label, opts.parallel);

    let mut tried: BTreeSet<u64> = BTreeSet::new();
    let mut running: Vec<Running> = Vec::new();
    let mut merged: Vec<u64> = Vec::new();
    let mut opened: Vec<u64> = Vec::new();
    let mut failed: Vec<u64> = Vec::new();
    loop {
        let all = candidates(&repo, &opts.label)?;
        let busy: Vec<Candidate> = running.iter().map(|r| r.candidate.clone()).collect();
        let waiting = ready(&all, &tried);
        for candidate in pick(&waiting, &busy, opts.parallel.saturating_sub(running.len())) {
            let log = logs.join(format!("issue-{}-{}.log", candidate.number, util::run_id()));
            let out = std::fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
            let err = out.try_clone()?;
            let mut command = Command::new(&program);
            command.args(["run", &candidate.number.to_string(), "--repo-dir"]).arg(&repo_dir);
            if opts.no_merge {
                command.arg("--no-merge");
            }
            let child = command.stdin(Stdio::null()).stdout(out).stderr(err).spawn().context("starting a run")?;
            println!("  #{} {} started. Log: {}", candidate.number, candidate.key, log.display());
            tried.insert(candidate.number);
            running.push(Running { candidate, child, log, started: Instant::now() });
        }
        if running.is_empty() {
            // Nothing runs and nothing can start. Say why for what is left.
            let left: Vec<&Candidate> = all.iter().filter(|c| !tried.contains(&c.number)).collect();
            for candidate in &left {
                let why = if candidate.held {
                    "carries the 'blocked' label, so a person decides first".to_string()
                } else {
                    let blockers: Vec<String> = candidate.blockers.iter().map(|n| format!("#{n}")).collect();
                    format!("is blocked by {}, still open", blockers.join(", "))
                };
                println!("  #{} {} not run: it {why}", candidate.number, candidate.key);
            }
            break;
        }

        // Wait for one run to end.
        let index = loop {
            let mut ended = None;
            for (index, run) in running.iter_mut().enumerate() {
                if run.child.try_wait()?.is_some() {
                    ended = Some(index);
                    break;
                }
            }
            match ended {
                Some(index) => break index,
                None => std::thread::sleep(Duration::from_millis(200)),
            }
        };
        let mut done = running.remove(index);
        let status = done.child.wait()?;
        let result = last_line(&done.log);
        let number = done.candidate.number;
        println!("  #{number} {} ended after {:.0} s: {result}", done.candidate.key, done.started.elapsed().as_secs_f64());
        if !status.success() {
            failed.push(number);
        } else if result.starts_with("MERGED") {
            merged.push(number);
            // GitHub closes the issue a moment after the merge. Issues it
            // blocks only become ready once it is closed.
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                let state = gh::api("GET", &format!("repos/{repo}/issues/{number}"), None)?;
                if state.get("state").and_then(Value::as_str) == Some("closed") {
                    break;
                }
                std::thread::sleep(pause());
            }
        } else {
            opened.push(number);
        }
    }

    let list = |numbers: &[u64]| if numbers.is_empty() { "none".to_string() } else { numbers.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ") };
    println!("queue done. Merged: {}. Pull request open: {}. Blocked: {}", list(&merged), list(&opened), list(&failed));
    // The day's handoff, written again with what just happened.
    match super::handoff::export(&super::handoff::HandoffOptions { date: None, out: None }) {
        Ok(files) => files.iter().for_each(|file| println!("handoff: {}", file.display())),
        Err(e) => eprintln!("warning: could not write the handoff: {e:#}"),
    }
    Ok(failed.is_empty())
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
