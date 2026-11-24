//! The local API: what the app and any other client read and ask for.
//!
//! Every route is under `/v1` and answers JSON. The engine returns data, not
//! layout: clients decide how things look. Clients poll. There is no event
//! stream yet.
//!
//! It listens on localhost unless told otherwise, because it can start runs.
//! It refuses any other address unless a token is set, and then every request
//! must carry that token.

use super::state::{IssueRow, Store};
use super::{editing, first_line, git, util};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct ServeOptions {
    pub bind: String,
    /// Required for any address that is not this machine.
    pub token: Option<String>,
    /// Checkouts of the repositories the API may act on and read workflows from.
    pub repo_dirs: Vec<PathBuf>,
}

/// A request, reduced to what the routes need.
pub struct Request<'a> {
    pub method: &'a str,
    /// The path without its query string.
    pub path: &'a str,
    pub query: BTreeMap<String, String>,
    pub body: &'a str,
}

/// What the server knows besides the database.
pub struct Context_ {
    pub home: PathBuf,
    /// `owner/name` to the checkout that holds it.
    pub repos: BTreeMap<String, PathBuf>,
    /// This program, for commands that start a run.
    pub program: PathBuf,
}

/// What a person would call the state of an issue, and what it waits on.
pub fn describe(row: &IssueRow) -> (&'static str, String) {
    let failure = row.failure.clone().unwrap_or_default();
    match (row.stage.as_str(), row.status.as_str()) {
        ("merged", _) => ("merged", String::new()),
        (_, "failed") => ("needs-a-person", first_line(failure.split("\n\nJudgment: ").next().unwrap_or(""))),
        (_, "dropped") => ("dropped", String::new()),
        (_, "waiting") => ("waiting", first_line(&failure)),
        ("github", "running") => ("waiting", "GitHub's checks and review".to_string()),
        ("github", "green") => ("pr-open", "a person to merge it".to_string()),
        ("pr", "open") => ("pr-open", "a person to look at it".to_string()),
        (_, "passed") => ("passed", String::new()),
        (_, "running") => ("running", String::new()),
        _ => ("unknown", String::new()),
    }
}

fn judgment_of(row: &IssueRow) -> Option<String> {
    row.failure.as_deref().and_then(|f| f.split_once("\n\nJudgment: ")).map(|(_, judgment)| judgment.trim().to_string())
}

fn issue_json(store: &Store, row: &IssueRow) -> Result<Value> {
    let (state, waits_on) = describe(row);
    let steps = store.steps(&row.repo, row.number, &row.run_id)?;
    let pass = steps.iter().map(|s| s.6).max().unwrap_or(0) + 1;
    Ok(json!({
        "repo": row.repo,
        "number": row.number,
        "key": row.key,
        "title": row.title,
        "state": state,
        "stage": row.stage,
        "status": row.status,
        "waits_on": waits_on,
        "pass": pass,
        "run_id": row.run_id,
        "head_sha": row.head_sha,
        "pr_url": row.pr_url,
        "judgment": judgment_of(row),
        "updated_at": row.updated_at,
    }))
}

fn run_dir(home: &Path, row: &IssueRow) -> PathBuf {
    home.join("runs").join(util::repo_dir_name(&row.repo)).join(format!("issue-{}", row.number)).join(&row.run_id)
}

fn detail_json(home: &Path, store: &Store, row: &IssueRow) -> Result<Value> {
    let mut value = issue_json(store, row)?;
    let steps: Vec<Value> = store
        .steps(&row.repo, row.number, &row.run_id)?
        .into_iter()
        .map(|(step, vendor, attempt, verdict, exit_code, duration_ms, pass)| {
            json!({ "step": step, "vendor": vendor, "attempt": attempt, "verdict": verdict, "exit_code": exit_code, "seconds": duration_ms as f64 / 1000.0, "pass": pass + 1 })
        })
        .collect();
    let dir = run_dir(home, row);
    let summary: Option<Value> = std::fs::read_to_string(dir.join("summary.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
    let mut files: Vec<String> = std::fs::read_dir(&dir).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    files.sort();
    let object = value.as_object_mut().ok_or_else(|| anyhow!("an issue must be an object"))?;
    object.insert("steps".into(), json!(steps));
    object.insert("summary".into(), json!(summary));
    object.insert("failure".into(), json!(row.failure.as_deref().map(|f| f.split("\n\nJudgment: ").next().unwrap_or("").trim().to_string())));
    object.insert("run_dir".into(), json!(dir.display().to_string()));
    object.insert("files".into(), json!(files));
    Ok(value)
}

/// The end of the file the run wrote to most recently: what the agent or
/// check that is running now has printed.
fn log_json(home: &Path, row: &IssueRow, file: Option<&str>) -> Value {
    let dir = run_dir(home, row);
    let chosen = match file {
        // A name from the client never leaves the run's folder.
        Some(name) if !name.contains('/') && !name.contains("..") => Some(dir.join(name)),
        Some(_) => None,
        None => std::fs::read_dir(&dir).ok().and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().ends_with(".log"))
                .filter(|e| e.metadata().map(|m| m.len() > 0).unwrap_or(false))
                // The newest file wins. An agent's stderr is preferred to its
                // stdout, which is one line of raw JSON.
                .max_by_key(|e| (e.metadata().and_then(|m| m.modified()).ok(), e.file_name().to_string_lossy().ends_with(".stderr.log")))
                .map(|e| e.path())
        }),
    };
    match chosen {
        Some(path) => {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let tail = lines[lines.len().saturating_sub(200)..].join("\n");
            let tail: String = tail.chars().rev().take(20_000).collect::<Vec<_>>().into_iter().rev().collect();
            json!({ "file": path.file_name().map(|n| n.to_string_lossy().to_string()), "text": tail })
        }
        None => json!({ "file": null, "text": "" }),
    }
}

/// Calls and time per vendor, and how often a usage limit was hit.
fn usage_json(store: &Store) -> Result<Value> {
    let mut by_vendor: BTreeMap<String, (u64, f64, u64)> = BTreeMap::new();
    for row in store.issues()? {
        for (_, vendor, _, verdict, _, duration_ms, _) in store.steps(&row.repo, row.number, &row.run_id)? {
            let Some(vendor) = vendor else { continue };
            let entry = by_vendor.entry(vendor).or_default();
            entry.0 += 1;
            entry.1 += duration_ms as f64 / 1000.0;
            if verdict == "rate-limited" {
                entry.2 += 1;
            }
        }
    }
    let list: Vec<Value> = by_vendor.into_iter().map(|(vendor, (calls, seconds, limited))| json!({ "vendor": vendor, "calls": calls, "seconds": seconds, "rate_limited": limited })).collect();
    Ok(json!(list))
}

fn find(store: &Store, owner: &str, name: &str, number: &str) -> Result<Option<IssueRow>> {
    let Ok(number) = number.parse::<u64>() else { return Ok(None) };
    store.issue(&format!("{owner}/{name}"), number)
}

/// Answers one request: a status code and a JSON body.
pub fn route(context: &Context_, request: &Request) -> (u16, Value) {
    let answer = (|| -> Result<(u16, Value)> {
        let store = Store::open(&context.home)?;
        let parts: Vec<&str> = request.path.trim_matches('/').split('/').collect();
        let not_found = || Ok((404, json!({ "error": format!("no such route: {} {}", request.method, request.path) })));
        Ok(match (request.method, parts.as_slice()) {
            ("GET", ["v1", "health"]) => (200, json!({ "ok": true, "version": env!("CARGO_PKG_VERSION"), "repos": context.repos.keys().collect::<Vec<_>>() })),
            ("GET", ["v1", "issues"]) => {
                let rows = store.issues()?;
                (200, json!(rows.iter().map(|row| issue_json(&store, row)).collect::<Result<Vec<_>>>()?))
            }
            ("GET", ["v1", "blocked"]) => {
                let rows: Vec<IssueRow> = store.issues()?.into_iter().filter(|r| r.status == "failed").collect();
                (200, json!(rows.iter().map(|row| issue_json(&store, row)).collect::<Result<Vec<_>>>()?))
            }
            ("GET", ["v1", "usage"]) => (200, usage_json(&store)?),
            ("GET", ["v1", "workflow"]) | ("GET", ["v1", "modules"]) | ("POST", ["v1", "workflow", "validate"]) | ("POST", ["v1", "workflow", "propose"]) | ("POST", ["v1", "modules"]) => {
                let repo = request.query.get("repo").cloned().or_else(|| context.repos.keys().next().cloned()).unwrap_or_default();
                let Some(dir) = context.repos.get(&repo) else {
                    return Ok((404, json!({ "error": format!("the server was not started with a checkout of '{repo}'") })));
                };
                let base = git::git(dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).unwrap_or_default();
                match (request.method, parts.as_slice()) {
                    ("GET", [_, "workflow"]) => (200, editing::current_graph(dir)?),
                    ("GET", [_, "modules"]) => (200, json!(editing::modules(dir)?)),
                    // A draft is checked exactly as a run would check it.
                    ("POST", [_, "workflow", "validate"]) => match editing::check(dir, request.body)? {
                        Ok(parsed) => (200, json!({ "ok": true, "errors": [], "workflow": editing::graph(&base, &parsed) })),
                        Err(problems) => (200, json!({ "ok": false, "errors": problems, "workflow": null })),
                    },
                    // Saving is a pull request. Nothing reaches the default branch from here.
                    ("POST", [_, "workflow", "propose"]) => match editing::propose_workflow(dir, request.body)? {
                        Ok(url) => (200, json!({ "ok": true, "url": url, "errors": [] })),
                        Err(problems) => (422, json!({ "ok": false, "url": null, "errors": problems })),
                    },
                    _ => {
                        let wanted: Value = serde_json::from_str(request.body).unwrap_or(Value::Null);
                        let Some(module) = wanted.get("module").and_then(Value::as_str) else {
                            return Ok((422, json!({ "ok": false, "error": "send {\"module\": \"<kind>/<name>\", \"role\": ...}" })));
                        };
                        match editing::propose_module(dir, module, wanted.get("role").and_then(Value::as_str)) {
                            Ok((url, files)) => (200, json!({ "ok": true, "url": url, "files": files })),
                            Err(e) => (422, json!({ "ok": false, "error": format!("{e:#}") })),
                        }
                    }
                }
            }
            ("GET", ["v1", "issues", owner, name, number]) => match find(&store, owner, name, number)? {
                Some(row) => (200, detail_json(&context.home, &store, &row)?),
                None => return not_found(),
            },
            ("GET", ["v1", "issues", owner, name, number, "log"]) => match find(&store, owner, name, number)? {
                Some(row) => (200, log_json(&context.home, &row, request.query.get("file").map(String::as_str))),
                None => return not_found(),
            },
            ("POST", ["v1", "issues", owner, name, number, action @ ("retry" | "drop")]) => {
                let repo = format!("{owner}/{name}");
                let (Some(row), Some(dir)) = (find(&store, owner, name, number)?, context.repos.get(&repo)) else {
                    return Ok((404, json!({ "error": format!("issue {repo}#{number} is unknown, or the server has no checkout of {repo}") })));
                };
                // The same commands a person types. A retry runs on its own,
                // with its output in the queue's log folder.
                let logs = context.home.join("queue").join(util::repo_dir_name(&repo));
                std::fs::create_dir_all(&logs)?;
                let log = logs.join(format!("issue-{}-{}-{action}.log", row.number, util::run_id()));
                let out = std::fs::File::create(&log)?;
                let mut command = Command::new(&context.program);
                command.args([*action, &row.number.to_string(), "--repo-dir"]).arg(dir);
                command.stdin(Stdio::null()).stdout(out.try_clone()?).stderr(out);
                if *action == "drop" {
                    let status = command.status().context("running factory drop")?;
                    (if status.success() { 200 } else { 500 }, json!({ "ok": status.success(), "log": log.display().to_string() }))
                } else {
                    command.spawn().context("starting factory retry")?;
                    (202, json!({ "ok": true, "started": true, "log": log.display().to_string() }))
                }
            }
            _ => return not_found(),
        })
    })();
    answer.unwrap_or_else(|e| (500, json!({ "error": format!("{e:#}") })))
}

fn parse_query(raw: &str) -> BTreeMap<String, String> {
    let decode = |text: &str| {
        let mut out = Vec::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'%' if i + 2 < bytes.len() && text.is_char_boundary(i + 1) && text.is_char_boundary(i + 3) => match u8::from_str_radix(&text[i + 1..i + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                },
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                other => {
                    out.push(other);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).to_string()
    };
    raw.split('&').filter(|pair| !pair.is_empty()).map(|pair| pair.split_once('=').unwrap_or((pair, ""))).map(|(k, v)| (decode(k), decode(v))).collect()
}

/// `owner/name` of a checkout, from its origin URL. No network.
pub fn repo_of(dir: &Path) -> Result<String> {
    let url = git::git(dir, &["remote", "get-url", "origin"])?;
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let mut parts = trimmed.rsplit(['/', ':']);
    let (Some(name), Some(owner)) = (parts.next(), parts.next()) else {
        bail!("can't read owner and name from the origin URL of {}", dir.display());
    };
    Ok(format!("{owner}/{name}"))
}

/// Whether a request may be served, given the token the server was started with.
pub fn authorized(token: Option<&str>, header: Option<&str>) -> bool {
    match token {
        None => true,
        Some(token) => header.and_then(|h| h.strip_prefix("Bearer ")).is_some_and(|given| given == token),
    }
}

pub fn serve(opts: &ServeOptions) -> Result<()> {
    let address: SocketAddr = opts.bind.parse().with_context(|| format!("'{}' is not an address like 127.0.0.1:7878", opts.bind))?;
    if !address.ip().is_loopback() && opts.token.as_deref().is_none_or(|t| t.trim().is_empty()) {
        bail!("refusing to listen on {address}: it is not this machine only, and no --token is set. This API can start runs");
    }
    let mut repos = BTreeMap::new();
    for dir in &opts.repo_dirs {
        let dir = dir.canonicalize().with_context(|| format!("{} does not exist", dir.display()))?;
        repos.insert(repo_of(&dir)?, dir);
    }
    let context = Context_ { home: util::home()?, repos, program: std::env::current_exe().context("finding the factory program")? };
    let server = tiny_http::Server::http(address).map_err(|e| anyhow!("could not listen on {address}: {e}"))?;
    println!("factory API on http://{address}/v1, for {}", if context.repos.is_empty() { "no repository (read only)".to_string() } else { context.repos.keys().cloned().collect::<Vec<_>>().join(", ") });
    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let method = request.method().as_str().to_string();
        let header = request.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.as_str().to_string());
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(request.as_reader(), &mut text);
        let (status, body) = if authorized(opts.token.as_deref(), header.as_deref()) {
            route(&context, &Request { method: &method, path, query: parse_query(query), body: &text })
        } else {
            (401, json!({ "error": "this server needs a token: send `Authorization: Bearer <token>`" }))
        };
        let response = tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("a valid header"));
        if let Err(e) = request.respond(response) {
            eprintln!("warning: could not answer {method} {path}: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::state::StepRecord;

    fn home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-api-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn step(store: &Store, issue: u64, name: &str, vendor: Option<&str>, verdict: &str, round: u32) {
        store
            .record(&StepRecord {
                ts: util::now(),
                repo: "o/r".into(),
                issue,
                run_id: "run1".into(),
                step: name.into(),
                kind: "agent".into(),
                module: format!("agent/{name}"),
                vendor: vendor.map(str::to_string),
                model: None,
                subscription: None,
                round,
                attempt: 1,
                sha: None,
                exit_code: Some(0),
                timed_out: false,
                verdict: verdict.into(),
                duration_ms: 30_000,
                input: None,
                output: None,
                stdout: None,
                stderr: None,
                session_id: None,
            })
            .unwrap();
    }

    fn get(context: &Context_, path: &str) -> (u16, Value) {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        route(context, &Request { method: "GET", path, query: parse_query(query), body: "" })
    }

    #[test]
    fn issues_are_described_for_a_person() {
        let home = home("issues");
        let store = Store::open(&home).unwrap();
        store.start_issue("o/r", 7, "trial", "a", "Add the file", "run1", "b", "/w", "abc").unwrap();
        step(&store, 7, "develop", Some("codex"), "replied", 0);
        step(&store, 7, "develop", Some("codex"), "rate-limited", 1);
        store.set_stage("o/r", 7, "verify", "failed", Some("3 fix rounds were used.\nlog\n\nJudgment: escalate. The simulator is broken.")).unwrap();
        store.start_issue("o/r", 8, "trial", "b", "Wait on CI", "run1", "b", "/w", "abc").unwrap();
        store.set_stage("o/r", 8, "github", "running", None).unwrap();
        let run = home.join("runs/o__r/issue-7/run1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("03-develop-1.stdout.log"), "first\nlast line\n").unwrap();
        std::fs::write(run.join("summary.json"), "{\"what_changed\":\"x\",\"how_it_went\":\"y\",\"for_a_human\":[]}").unwrap();
        let context = Context_ { home: home.clone(), repos: BTreeMap::new(), program: PathBuf::from("factory") };

        let (status, list) = get(&context, "/v1/issues");
        assert_eq!(status, 200);
        let by_number = |n: u64| list.as_array().unwrap().iter().find(|i| i["number"] == n).unwrap().clone();
        assert_eq!(by_number(7)["state"], "needs-a-person");
        assert_eq!(by_number(7)["waits_on"], "3 fix rounds were used.");
        assert_eq!(by_number(7)["judgment"], "escalate. The simulator is broken.");
        assert_eq!(by_number(7)["pass"], 2);
        assert_eq!((by_number(8)["state"].as_str(), by_number(8)["waits_on"].as_str()), (Some("waiting"), Some("GitHub's checks and review")));

        let (_, blocked) = get(&context, "/v1/blocked");
        assert_eq!(blocked.as_array().unwrap().len(), 1);

        let (status, detail) = get(&context, "/v1/issues/o/r/7");
        assert_eq!(status, 200);
        assert_eq!(detail["steps"].as_array().unwrap().len(), 2);
        assert_eq!(detail["steps"][1]["pass"], 2);
        assert_eq!(detail["summary"]["what_changed"], "x");
        assert_eq!(detail["failure"], "3 fix rounds were used.\nlog");
        assert!(detail["files"].as_array().unwrap().iter().any(|f| f == "summary.json"));

        let (_, log) = get(&context, "/v1/issues/o/r/7/log");
        assert_eq!((log["file"].as_str(), log["text"].as_str()), (Some("03-develop-1.stdout.log"), Some("first\nlast line")));
        // A file name from the client can't leave the run's folder.
        assert_eq!(get(&context, "/v1/issues/o/r/7/log?file=..%2F..%2Fstate.db").1["file"], Value::Null);

        let (_, usage) = get(&context, "/v1/usage");
        assert_eq!(usage[0], json!({ "vendor": "codex", "calls": 2, "seconds": 60.0, "rate_limited": 1 }));

        assert_eq!(get(&context, "/v1/issues/o/r/99").0, 404);
        assert_eq!(get(&context, "/v2/issues").0, 404);
        assert_eq!(get(&context, "/v1/workflow?repo=o/r").0, 404);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_token_is_all_or_nothing() {
        assert!(authorized(None, None));
        assert!(authorized(Some("s3cret"), Some("Bearer s3cret")));
        assert!(!authorized(Some("s3cret"), None));
        assert!(!authorized(Some("s3cret"), Some("Bearer wrong")));
        assert!(!authorized(Some("s3cret"), Some("s3cret")));
        // Anywhere but this machine needs a token.
        let open = ServeOptions { bind: "0.0.0.0:7878".into(), token: None, repo_dirs: Vec::new() };
        assert!(serve(&open).unwrap_err().to_string().contains("refusing to listen on 0.0.0.0:7878"));
    }

    #[test]
    fn queries_are_decoded() {
        let query = parse_query("repo=o%2Fr&file=a+b.log&flag");
        assert_eq!(query.get("repo").map(String::as_str), Some("o/r"));
        assert_eq!(query.get("file").map(String::as_str), Some("a b.log"));
        assert_eq!(query.get("flag").map(String::as_str), Some(""));
    }
}
