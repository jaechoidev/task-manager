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

fn launchctl(args: &[&str]) -> Result<(bool, String)> {
    let output = Command::new("launchctl").args(args).stdin(Stdio::null()).output().context("running launchctl")?;
    Ok((output.status.success(), format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))))
}

/// The process of a loaded agent, from what `launchctl print` says.
pub fn pid_of(print: &str) -> Option<u32> {
    print.lines().map(str::trim).find_map(|line| line.strip_prefix("pid = ")).and_then(|pid| pid.trim().parse().ok())
}

fn state_of(job: &Job, file: &Path) -> Result<State> {
    let (loaded, print) = launchctl(&["print", &format!("{}/{}", domain()?, job.label)])?;
    Ok(State { label: job.label.clone(), installed: file.is_file(), loaded, pid: if loaded { pid_of(&print) } else { None }, log: job.log.clone() })
}

/// Writes both agents and starts them. An agent that is already there is
/// stopped first and started with the new settings.
pub fn install(opts: &Options) -> Result<Vec<State>> {
    let home = util::home()?;
    if Config::load(&home)?.projects.is_empty() {
        bail!("no project is registered, so the scheduler would have nothing to watch. Add one with `factory project add <checkout>`");
    }
    let program = std::env::current_exe().context("finding the factory program")?;
    let path = std::env::var("PATH").map_err(|_| anyhow!("PATH is not set, so the service could not find git, gh or the agent programs"))?;
    let factory_home = std::env::var_os("FACTORY_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from);
    let dir = agents_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::create_dir_all(home.join("service"))?;
    let domain = domain()?;
    let mut states = Vec::new();
    for job in jobs(&home, opts) {
        let file = dir.join(format!("{}.plist", job.label));
        // Loaded with other settings: stop it, so the new file is what runs.
        let _ = launchctl(&["bootout", &format!("{domain}/{}", job.label)]);
        std::fs::write(&file, plist(&job, &program, &path, factory_home.as_deref())).with_context(|| format!("writing {}", file.display()))?;
        let (ok, said) = launchctl(&["bootstrap", &domain, &file.display().to_string()])?;
        if !ok {
            bail!("launchd did not take {}: {}", file.display(), said.trim());
        }
        states.push(state_of(&job, &file)?);
    }
    Ok(states)
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
        let (stopped, _) = launchctl(&["bootout", &format!("{domain}/{}", job.label)])?;
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
        .map(|job| state_of(job, &dir.join(format!("{}.plist", job.label))))
        .collect()
}

pub fn print(states: &[State]) {
    for state in states {
        let how = match (state.installed, state.loaded, state.pid) {
            (_, true, Some(pid)) => format!("running, process {pid}"),
            (_, true, None) => "loaded, not running right now. launchd starts it again".to_string(),
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

    #[test]
    fn the_process_is_read_from_what_launchd_prints() {
        let running = "gui/501/dev.factory.serve = {\n\tactive count = 1\n\tstate = running\n\n\tprogram = /x/factory\n\tpid = 4242\n\timmediate reason = speculative\n}";
        assert_eq!(pid_of(running), Some(4242));
        let waiting = "gui/501/dev.factory.serve = {\n\tstate = spawn scheduled\n\tlast exit code = 1\n}";
        assert_eq!(pid_of(waiting), None);
    }
}
