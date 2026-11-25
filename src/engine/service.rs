//! The scheduler and the API as services of this Mac: two launchd agents
//! that start at login and are started again when they stop.
//!
//! Until now every run began because somebody started a process. With the
//! service, labelling an issue is all it takes: `factory schedule --watch`
//! is always there to see it, and `factory serve` is always there for the
//! app.
//!
//! Each agent is a property list in `~/Library/LaunchAgents`. Its
//! environment is only what is written there: the PATH the programs are
//! found on, and the factory home when one is set. Nothing else of the
//! shell that installed it is carried over, so the agents' calls use this
//! machine's own logins and never a login that shell happened to hold.

use super::config::Config;
use super::util;
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// How the scheduler and the API are to be run.
pub struct Options {
    pub parallel: Option<usize>,
    pub in_order: bool,
    pub no_merge: bool,
    pub no_guard: bool,
    pub bind: String,
}

/// One agent: what launchd calls it, what it runs, and where its output goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub label: String,
    pub args: Vec<String>,
    pub log: PathBuf,
    /// Builds and simulators need the machine's full attention. The API does not.
    pub heavy: bool,
}

/// What launchd says about an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub label: String,
    /// Whether its property list is in place.
    pub installed: bool,
    /// Whether launchd has it loaded.
    pub loaded: bool,
    /// The process, while it runs.
    pub pid: Option<u32>,
    pub log: PathBuf,
    /// For the API: where it listens, and whether it answered there.
    pub api: Option<Api>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Api {
    pub address: String,
    pub answer: Answer,
}

/// Who answers on the API's address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// This agent's own process.
    Ours,
    /// Another server, with its process when it said which.
    Someone(Option<u32>),
    Nobody,
}

/// Who a health reply came from, given the agent's process.
fn answer(reply: Option<Option<u32>>, agent: Option<u32>) -> Answer {
    match reply {
        None => Answer::Nobody,
        Some(Some(pid)) if Some(pid) == agent => Answer::Ours,
        Some(pid) => Answer::Someone(pid),
    }
}

/// `dev.factory`, with a suffix when a test asks for agents of its own.
fn prefix() -> String {
    match std::env::var("FACTORY_SERVICE_SUFFIX").ok().filter(|suffix| !suffix.is_empty()) {
        Some(suffix) => format!("dev.factory.{suffix}"),
        None => "dev.factory".to_string(),
    }
}

/// Where the property lists go. A file elsewhere is loaded for this login
/// only, which is what a test wants.
fn agents_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FACTORY_SERVICE_DIR").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents"))
}

/// The two agents for these options.
pub fn jobs(home: &Path, opts: &Options) -> Vec<Job> {
    let mut schedule = vec!["schedule".to_string(), "--watch".to_string()];
    if let Some(parallel) = opts.parallel {
        schedule.extend(["--parallel".to_string(), parallel.to_string()]);
    }
    for (on, flag) in [(opts.in_order, "--in-order"), (opts.no_merge, "--no-merge"), (opts.no_guard, "--no-guard")] {
        if on {
            schedule.push(flag.to_string());
        }
    }
    let logs = home.join("service");
    vec![
        Job { label: format!("{}.schedule", prefix()), args: schedule, log: logs.join("schedule.log"), heavy: true },
        Job { label: format!("{}.serve", prefix()), args: vec!["serve".to_string(), "--bind".to_string(), opts.bind.clone()], log: logs.join("serve.log"), heavy: false },
    ]
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The property list of one agent. `path` is the PATH its programs are
/// found on, and `factory_home` is set only when the installer had one set.
pub fn plist(job: &Job, program: &Path, path: &str, factory_home: Option<&Path>) -> String {
    let string = |text: &str| format!("<string>{}</string>", escape(text));
    let arguments: String = std::iter::once(program.display().to_string()).chain(job.args.iter().cloned()).map(|argument| format!("    {}\n", string(&argument))).collect();
    let mut environment = format!("    <key>PATH</key>{}\n", string(path));
    if let Some(home) = factory_home {
        environment.push_str(&format!("    <key>FACTORY_HOME</key>{}\n", string(&home.display().to_string())));
    }
    let log = string(&job.log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>{label}
  <key>ProgramArguments</key>
  <array>
{arguments}  </array>
  <key>EnvironmentVariables</key>
  <dict>
{environment}  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>ProcessType</key>{kind}
  <key>StandardOutPath</key>{log}
  <key>StandardErrorPath</key>{log}
</dict>
</plist>
"#,
        label = string(&job.label),
        kind = string(if job.heavy { "Interactive" } else { "Standard" }),
    )
}

/// This user's launchd domain, where an agent of the login session lives.
fn domain() -> Result<String> {
    let output = Command::new("id").arg("-u").output().context("asking for the user id")?;
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() || !uid.chars().all(|c| c.is_ascii_digit()) {
        bail!("could not read the user id");
    }
    Ok(format!("gui/{uid}"))
}

/// What one `launchctl` call answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    pub ok: bool,
    pub text: String,
}

fn launchctl(args: &[&str]) -> Result<Said> {
    let output = Command::new("launchctl").args(args).stdin(Stdio::null()).output().context("running launchctl")?;
    Ok(Said { ok: output.status.success(), text: format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)) })
}

/// How long launchd may take to finish removing an agent, and how often to look.
const GONE_WITHIN: Duration = Duration::from_secs(10);
const LOOK_EVERY: Duration = Duration::from_millis(250);
/// How many times to offer launchd the new file, a second apart.
const TAKE_TRIES: u32 = 5;

/// launchctl's own text, without its advice to run as root, which is wrong
/// for an agent of the login session.
fn plain(text: &str) -> String {
    text.lines().filter(|line| !line.contains("re-running the command as root")).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// Stops the agent if it is loaded and starts it from `file`.
///
/// `bootout` returns before launchd has finished removing the agent, and a
/// `bootstrap` of a label still being removed fails with "5: Input/output
/// error". So this waits until launchd no longer knows the label, and offers
/// the file again a few times if launchd still refuses it.
fn restart(target: &str, domain: &str, file: &Path, run: &mut dyn FnMut(&[&str]) -> Result<Said>, wait: &mut dyn FnMut(Duration)) -> Result<std::result::Result<(), String>> {
    run(&["bootout", target])?;
    let mut failed = None;
    let mut gone = || match run(&["print", target]) {
        Ok(said) => !said.ok,
        Err(e) => {
            failed = Some(e);
            true
        }
    };
    within(GONE_WITHIN, &mut gone, wait);
    if let Some(e) = failed {
        return Err(e);
    }
    let file = file.display().to_string();
    let mut last = String::new();
    for attempt in 1..=TAKE_TRIES {
        let said = run(&["bootstrap", domain, &file])?;
        if said.ok {
            return Ok(Ok(()));
        }
        last = plain(&said.text);
        if attempt < TAKE_TRIES {
            wait(Duration::from_secs(1));
        }
    }
    Ok(Err(format!("launchd did not take {file} after {TAKE_TRIES} tries: {last}")))
}

/// The PATH with each folder once, where it first appears, and no empty
/// entries. The first of two copies is the one that was found first anyway.
pub fn tidy_path(path: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    path.split(':').filter(|dir| !dir.is_empty() && seen.insert(*dir)).collect::<Vec<_>>().join(":")
}

/// The process of a loaded agent, from what `launchctl print` says.
pub fn pid_of(print: &str) -> Option<u32> {
    print.lines().map(str::trim).find_map(|line| line.strip_prefix("pid = ")).and_then(|pid| pid.trim().parse().ok())
}

fn state_of(job: &Job, file: &Path) -> Result<State> {
    let print = launchctl(&["print", &format!("{}/{}", domain()?, job.label)])?;
    Ok(State { label: job.label.clone(), installed: file.is_file(), loaded: print.ok, pid: if print.ok { pid_of(&print.text) } else { None }, log: job.log.clone(), api: None })
}

/// The address an agent's API listens on, from its arguments or its file.
fn bind_of(args: &[String]) -> Option<String> {
    args.iter().position(|arg| arg == "--bind").and_then(|at| args.get(at + 1)).filter(|address| !address.is_empty()).cloned()
}

fn bind_in(plist: &str) -> Option<String> {
    let args: Vec<String> = plist.split("<string>").skip(1).filter_map(|piece| piece.split_once("</string>")).map(|(text, _)| text.to_string()).collect();
    bind_of(&args)
}

/// How long a newly started API has to answer.
const ANSWERS_WITHIN: Duration = Duration::from_secs(10);

/// Asks `address` for `/v1/health`. `None` when nothing answers over HTTP.
/// Otherwise the process the reply names: an API says its own, and a
/// server that wants a token, or isn't factory's, names none.
pub fn health(address: &str, within: Duration) -> Option<Option<u32>> {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    let socket = address.to_socket_addrs().ok()?.next()?;
    let mut stream = std::net::TcpStream::connect_timeout(&socket, within).ok()?;
    let _ = stream.set_read_timeout(Some(within));
    let _ = stream.set_write_timeout(Some(within));
    stream.write_all(format!("GET /v1/health HTTP/1.0\r\nHost: {address}\r\n\r\n").as_bytes()).ok()?;
    let mut reply = Vec::new();
    // HTTP/1.0: the server closes when it is done. What came before a
    // timeout still counts.
    let _ = stream.take(64 * 1024).read_to_end(&mut reply);
    let reply = String::from_utf8_lossy(&reply);
    if !reply.starts_with("HTTP/1.") {
        return None;
    }
    let body = reply.split_once("\r\n\r\n").map(|(_, body)| body).unwrap_or_default();
    let pid = serde_json::from_str::<serde_json::Value>(body).ok().and_then(|value| value.get("pid")?.as_u64()).and_then(|pid| u32::try_from(pid).ok());
    Some(pid)
}

/// Looks until `check` is true, for at most `limit`. The time a look takes
/// counts too: asking a port that accepts and never replies takes the whole
/// timeout of the ask.
fn within(limit: Duration, check: &mut dyn FnMut() -> bool, wait: &mut dyn FnMut(Duration)) -> bool {
    let mut spent = Duration::ZERO;
    loop {
        let started = std::time::Instant::now();
        let done = check();
        spent += started.elapsed();
        if done {
            return true;
        }
        if spent >= limit {
            return false;
        }
        wait(LOOK_EVERY);
        spent += LOOK_EVERY;
    }
}

/// What `install` left: each agent's state, and why any was not taken.
pub struct Installed {
    pub states: Vec<State>,
    pub refused: Vec<String>,
}

/// Writes both agents and starts them. An agent that is already there is
/// stopped first and started with the new settings. One that launchd
/// refuses doesn't stop the other from being tried.
pub fn install(opts: &Options) -> Result<Installed> {
    let home = util::home()?;
    if Config::load(&home)?.projects.is_empty() {
        bail!("no project is registered, so the scheduler would have nothing to watch. Add one with `factory project add <checkout>`");
    }
    let program = std::env::current_exe().context("finding the factory program")?;
    let path = tidy_path(&std::env::var("PATH").map_err(|_| anyhow!("PATH is not set, so the service could not find git, gh or the agent programs"))?);
    let factory_home = std::env::var_os("FACTORY_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from);
    let dir = agents_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::create_dir_all(home.join("service"))?;
    let domain = domain()?;
    let jobs = jobs(&home, opts);
    // Every file is written before anything is stopped, so a file that can't
    // be written leaves what runs as it was.
    for job in &jobs {
        let file = dir.join(format!("{}.plist", job.label));
        std::fs::write(&file, plist(job, &program, &path, factory_home.as_deref())).with_context(|| format!("writing {}", file.display()))?;
    }
    let mut states = Vec::new();
    let mut refused = Vec::new();
    for job in &jobs {
        let file = dir.join(format!("{}.plist", job.label));
        // Loaded with other settings: stop it, so the new file is what runs.
        let target = format!("{domain}/{}", job.label);
        let taken = restart(&target, &domain, &file, &mut launchctl, &mut std::thread::sleep)?;
        if let Err(why) = &taken {
            refused.push(format!("{}: {why}", job.label));
        }
        // launchd has a process before the API listens, and another server
        // on the same address answers at once. So it counts only when the
        // answer names this agent's own process.
        let mut state = state_of(job, &file)?;
        if let Some(address) = bind_of(&job.args).filter(|_| taken.is_ok()) {
            let mut seen = Answer::Nobody;
            let mut look = || {
                let Ok(now) = state_of(job, &file) else { return false };
                seen = answer(health(&address, Duration::from_secs(1)), now.pid);
                state = now;
                seen == Answer::Ours
            };
            within(ANSWERS_WITHIN, &mut look, &mut std::thread::sleep);
            let after = ANSWERS_WITHIN.as_secs();
            match seen {
                Answer::Ours => {}
                Answer::Nobody => refused.push(format!("{}: loaded, but nothing answers on http://{address}/v1/health after {after} s. Its output is in {}", job.label, job.log.display())),
                Answer::Someone(other) => {
                    let who = other.map(|pid| format!("process {pid}")).unwrap_or_else(|| "another server".to_string());
                    refused.push(format!("{}: {who} answers on http://{address}, not this agent, after {after} s. Stop it, for example a `factory serve` started by hand, and install again. This agent's output is in {}", job.label, job.log.display()));
                }
            }
            state.api = Some(Api { address, answer: seen });
        }
        states.push(state);
    }
    Ok(Installed { states, refused })
}

/// Stops both agents and removes their files. Issues at work are cut off
/// where they are and pick up at that step the next time they are run.
pub fn uninstall() -> Result<Vec<String>> {
    let home = util::home()?;
    let dir = agents_dir()?;
    let domain = domain()?;
    let mut done = Vec::new();
    for job in jobs(&home, &Options { parallel: None, in_order: false, no_merge: false, no_guard: false, bind: String::new() }) {
        let file = dir.join(format!("{}.plist", job.label));
        let stopped = launchctl(&["bootout", &format!("{domain}/{}", job.label)])?.ok;
        let removed = std::fs::remove_file(&file).is_ok();
        done.push(match (stopped, removed) {
            (true, _) => format!("{}: stopped and removed", job.label),
            (false, true) => format!("{}: was not running. Removed", job.label),
            (false, false) => format!("{}: was not installed", job.label),
        });
    }
    Ok(done)
}

pub fn status() -> Result<Vec<State>> {
    let home = util::home()?;
    let dir = agents_dir()?;
    jobs(&home, &Options { parallel: None, in_order: false, no_merge: false, no_guard: false, bind: String::new() })
        .iter()
        .map(|job| {
            let file = dir.join(format!("{}.plist", job.label));
            let mut state = state_of(job, &file)?;
            // The address is the one the installed file names.
            if let Some(address) = std::fs::read_to_string(&file).ok().and_then(|text| bind_in(&text)) {
                state.api = Some(Api { answer: answer(health(&address, Duration::from_secs(1)), state.pid), address });
            }
            Ok(state)
        })
        .collect()
}

/// Whether both agents run, and the API answers as its own agent.
pub fn healthy(states: &[State]) -> bool {
    states.iter().all(|state| state.loaded && state.pid.is_some() && state.api.as_ref().is_none_or(|api| api.answer == Answer::Ours))
}

pub fn print(states: &[State]) {
    for state in states {
        let how = match (state.installed, state.loaded, state.pid) {
            (_, true, Some(pid)) => match &state.api {
                Some(Api { address, answer: Answer::Ours }) => format!("running, process {pid}, answering on http://{address}"),
                Some(Api { address, answer: Answer::Nobody }) => format!("running, process {pid}, NOT answering on http://{address}"),
                Some(Api { address, answer: Answer::Someone(Some(other)) }) => format!("running, process {pid}, but process {other} answers on http://{address}"),
                Some(Api { address, answer: Answer::Someone(None) }) => format!("running, process {pid}, but another server answers on http://{address}"),
                None => format!("running, process {pid}"),
            },
            (_, true, None) => match &state.api {
                Some(Api { address, answer: Answer::Someone(Some(other)) }) => format!("loaded, not running right now. Process {other} answers on http://{address}, so this agent can't listen there"),
                Some(Api { address, answer: Answer::Someone(None) }) => format!("loaded, not running right now. Another server answers on http://{address}, so this agent can't listen there"),
                _ => "loaded, not running right now. launchd starts it again".to_string(),
            },
            (true, false, _) => "installed, not loaded. It starts at the next login, or with `factory service install`".to_string(),
            (false, false, _) => "not installed".to_string(),
        };
        println!("{}: {how}", state.label);
        if state.installed || state.loaded {
            println!("  output: {}", state.log.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agents_run_the_watching_scheduler_and_the_api() {
        let opts = Options { parallel: Some(2), in_order: true, no_merge: false, no_guard: true, bind: "127.0.0.1:7878".into() };
        let jobs = jobs(Path::new("/home/.factory"), &opts);
        assert_eq!(jobs[0].label, "dev.factory.schedule");
        assert_eq!(jobs[0].args, ["schedule", "--watch", "--parallel", "2", "--in-order", "--no-guard"]);
        assert_eq!((jobs[0].log.as_path(), jobs[0].heavy), (Path::new("/home/.factory/service/schedule.log"), true));
        assert_eq!(jobs[1].label, "dev.factory.serve");
        assert_eq!(jobs[1].args, ["serve", "--bind", "127.0.0.1:7878"]);
        // With nothing asked for, the scheduler decides for itself.
        let plain = super::jobs(Path::new("/h"), &Options { parallel: None, in_order: false, no_merge: false, no_guard: false, bind: "b".into() });
        assert_eq!(plain[0].args, ["schedule", "--watch"]);
    }

    #[test]
    fn a_property_list_carries_only_the_path_and_the_factory_home() {
        let job = Job { label: "dev.factory.schedule".into(), args: vec!["schedule".into(), "--watch".into()], log: PathBuf::from("/h/service/schedule.log"), heavy: true };
        let text = plist(&job, Path::new("/opt/My Tools/factory"), "/usr/bin:/a&b", Some(Path::new("/h <x>")));
        assert!(text.contains("<key>Label</key><string>dev.factory.schedule</string>"), "{text}");
        assert!(text.contains("    <string>/opt/My Tools/factory</string>\n    <string>schedule</string>\n    <string>--watch</string>\n"), "{text}");
        // What a property list can't hold as it is, is escaped.
        assert!(text.contains("<key>PATH</key><string>/usr/bin:/a&amp;b</string>"), "{text}");
        assert!(text.contains("<key>FACTORY_HOME</key><string>/h &lt;x&gt;</string>"), "{text}");
        assert!(text.contains("<key>KeepAlive</key><true/>") && text.contains("<key>RunAtLoad</key><true/>"), "{text}");
        assert!(text.contains("<key>ProcessType</key><string>Interactive</string>"), "{text}");
        // No login of the installing shell goes along: these two keys are all.
        assert_eq!(text.matches("<key>").count(), 11, "{text}");
        let light = plist(&Job { heavy: false, ..job }, Path::new("/f"), "/bin", None);
        assert!(light.contains("<string>Standard</string>") && !light.contains("FACTORY_HOME"), "{light}");
    }

    type Calls = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// A launchctl that answers from a script, and the calls it was given.
    fn scripted(answers: Vec<(&'static str, bool, &'static str)>) -> (impl FnMut(&[&str]) -> Result<Said>, Calls) {
        let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = calls.clone();
        let mut answers = answers.into_iter();
        let run = move |args: &[&str]| {
            seen.borrow_mut().push(args[0].to_string());
            let (verb, ok, text) = answers.next().expect("an answer for every call");
            assert_eq!(verb, args[0], "calls so far: {:?}", seen.borrow());
            Ok(Said { ok, text: text.to_string() })
        };
        (run, calls)
    }

    const REFUSED: &str = "Bootstrap failed: 5: Input/output error\nTry re-running the command as root for richer errors.\n";

    #[test]
    fn a_restart_waits_for_the_old_agent_to_go_and_offers_the_file_again() {
        // launchd still knows the label twice after bootout, then refuses the
        // file once, as it does while the old agent is being removed.
        let (mut run, calls) = scripted(vec![("bootout", true, ""), ("print", true, "state = running"), ("print", true, "state = exiting"), ("print", false, ""), ("bootstrap", false, REFUSED), ("bootstrap", true, "")]);
        let mut waits = Vec::new();
        let taken = restart("gui/501/dev.factory.serve", "gui/501", Path::new("/a/serve.plist"), &mut run, &mut |d| waits.push(d)).unwrap();
        assert_eq!(taken, Ok(()));
        assert_eq!(*calls.borrow(), ["bootout", "print", "print", "print", "bootstrap", "bootstrap"]);
        assert_eq!(waits, [LOOK_EVERY, LOOK_EVERY, Duration::from_secs(1)]);
    }

    #[test]
    fn a_file_launchd_keeps_refusing_is_reported_without_launchctls_advice() {
        let mut answers = vec![("bootout", false, "Boot-out failed: 3: No such process"), ("print", false, "")];
        answers.extend(std::iter::repeat_n(("bootstrap", false, REFUSED), TAKE_TRIES as usize));
        let (mut run, calls) = scripted(answers);
        let why = restart("gui/501/x", "gui/501", Path::new("/a/x.plist"), &mut run, &mut |_| {}).unwrap().unwrap_err();
        assert_eq!(why, format!("launchd did not take /a/x.plist after {TAKE_TRIES} tries: Bootstrap failed: 5: Input/output error"));
        assert_eq!(calls.borrow().iter().filter(|c| *c == "bootstrap").count(), TAKE_TRIES as usize);
    }

    #[test]
    fn an_agent_that_never_goes_is_waited_for_only_so_long() {
        let looks = (GONE_WITHIN.as_millis() / LOOK_EVERY.as_millis()) as usize;
        let mut answers = vec![("bootout", true, "")];
        answers.extend(std::iter::repeat_n(("print", true, "state = running"), looks + 1));
        answers.push(("bootstrap", true, ""));
        let (mut run, _) = scripted(answers);
        let mut waited = Duration::ZERO;
        assert_eq!(restart("t", "d", Path::new("/f"), &mut run, &mut |d| waited += d).unwrap(), Ok(()));
        assert_eq!(waited, GONE_WITHIN);
    }

    #[test]
    fn the_api_address_is_read_from_the_arguments_or_the_installed_file() {
        let opts = Options { parallel: None, in_order: false, no_merge: false, no_guard: false, bind: "127.0.0.1:7878".into() };
        let jobs = jobs(Path::new("/h"), &opts);
        assert_eq!((bind_of(&jobs[0].args), bind_of(&jobs[1].args)), (None, Some("127.0.0.1:7878".to_string())));
        let file = plist(&jobs[1], Path::new("/bin/factory"), "/bin", None);
        assert_eq!(bind_in(&file), Some("127.0.0.1:7878".to_string()));
        assert_eq!(bind_in(&plist(&jobs[0], Path::new("/bin/factory"), "/bin", None)), None);
    }

    /// A server that answers one request with `reply`, and the request it got.
    fn one_reply(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 64];
            let _ = stream.read(&mut request);
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).to_string()
        });
        (address, server)
    }

    #[test]
    fn a_health_reply_names_the_process_that_answered() {
        let (address, server) = one_reply("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true,\"pid\":4242}");
        assert_eq!(health(&address, Duration::from_secs(2)), Some(Some(4242)));
        assert!(server.join().unwrap().starts_with("GET /v1/health HTTP/1.0\r\n"));
        // One that wants a token is up, and doesn't say who it is.
        let (address, server) = one_reply("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
        assert_eq!(health(&address, Duration::from_secs(2)), Some(None));
        server.join().unwrap();
        // Nobody listens there now.
        assert_eq!(health(&address, Duration::from_millis(500)), None);
        assert_eq!(health("not an address", Duration::from_millis(100)), None);
    }

    #[test]
    fn only_an_answer_from_the_agents_own_process_counts() {
        assert_eq!(answer(Some(Some(7)), Some(7)), Answer::Ours);
        // A server started by hand on the same address, while the agent's
        // own process is still starting or already gone.
        assert_eq!(answer(Some(Some(9)), Some(7)), Answer::Someone(Some(9)));
        assert_eq!(answer(Some(Some(9)), None), Answer::Someone(Some(9)));
        assert_eq!(answer(Some(None), Some(7)), Answer::Someone(None));
        assert_eq!(answer(None, Some(7)), Answer::Nobody);
    }

    #[test]
    fn a_new_api_is_given_a_while_to_answer() {
        let mut asked = 0;
        let mut waited = Duration::ZERO;
        assert!(within(ANSWERS_WITHIN, &mut || { asked += 1; asked == 3 }, &mut |d| waited += d));
        assert_eq!((asked, waited), (3, LOOK_EVERY * 2));
        let mut waited = Duration::ZERO;
        assert!(!within(ANSWERS_WITHIN, &mut || false, &mut |d| waited += d));
        assert_eq!(waited, ANSWERS_WITHIN);
    }

    #[test]
    fn a_slow_look_counts_toward_the_limit() {
        // Each look takes 100 ms, as an ask of a port that never replies
        // takes its whole timeout. With 250 ms between looks, a 400 ms limit
        // allows two, not the three that counting only the pauses would.
        let mut looks = 0;
        let slow = &mut || {
            looks += 1;
            std::thread::sleep(Duration::from_millis(100));
            false
        };
        assert!(!within(Duration::from_millis(400), slow, &mut |_| {}));
        assert_eq!(looks, 2);
    }

    #[test]
    fn healthy_means_both_run_and_the_api_answers_as_itself() {
        let state = |pid: Option<u32>, answer: Option<Answer>| State { label: "x".into(), installed: true, loaded: true, pid, log: PathBuf::from("/l"), api: answer.map(|answer| Api { address: "a".into(), answer }) };
        assert!(healthy(&[state(Some(1), None), state(Some(2), Some(Answer::Ours))]));
        assert!(!healthy(&[state(Some(1), None), state(None, Some(Answer::Someone(Some(9))))]));
        assert!(!healthy(&[state(Some(1), None), state(Some(2), Some(Answer::Nobody))]));
        assert!(!healthy(&[state(None, None), state(Some(2), Some(Answer::Ours))]));
        assert!(!healthy(&[State { loaded: false, ..state(None, None) }]));
    }

    #[test]
    fn the_path_keeps_each_folder_once_where_it_first_appears() {
        assert_eq!(tidy_path("/a/bin:/usr/bin::/h/.local/bin:/usr/bin:/h/.local/bin"), "/a/bin:/usr/bin:/h/.local/bin");
    }

    #[test]
    fn the_process_is_read_from_what_launchd_prints() {
        let running = "gui/501/dev.factory.serve = {\n\tactive count = 1\n\tstate = running\n\n\tprogram = /x/factory\n\tpid = 4242\n\timmediate reason = speculative\n}";
        assert_eq!(pid_of(running), Some(4242));
        let waiting = "gui/501/dev.factory.serve = {\n\tstate = spawn scheduled\n\tlast exit code = 1\n}";
        assert_eq!(pid_of(waiting), None);
    }
}
