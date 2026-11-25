//! What this machine can run at once.
//!
//! The steps of a run need different things. Agent calls use almost no local
//! CPU and are limited by a subscription. A check that builds the app and
//! drives a simulator needs cores and memory. This reads what the machine
//! has, checks the tools a run depends on, and proposes how many of each
//! kind of step to allow at a time.
//!
//! It only reads. Nothing is changed and nothing is started.

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::process::Command;

// The numbers below come from one measurement: the trial app's verify on a
// 12-core, 96 GB M2 Max, run 1, 2 and 3 at once on separate simulators. See
// docs/findings/2026-11-24-capacity.md. The trial app builds in seconds. A
// project with a long build needs more cores per step than this assumes.

/// Cores one simulator step is given: an `xcodebuild`, then a simulator and
/// Maestro's driver. Three at once on 12 cores cost each run 12% more time.
pub const CORES_PER_SIMULATOR_STEP: f64 = 4.0;
/// Memory one booted simulator held, in GB, with what it caches.
pub const GB_PER_SIMULATOR_STEP: f64 = 8.0;
/// Memory left for everything else on the machine, in GB.
pub const GB_RESERVED: f64 = 8.0;
/// No more simulator steps at once than this, however large the machine.
/// It is the most that has been measured.
pub const MAX_SIMULATOR_STEPS: u32 = 3;
/// Busy cores that do not count against the allowance.
pub const BUSY_CORES_IGNORED: f64 = 1.0;
/// Disk one warmed-up simulator took, in GB.
pub const GB_DISK_PER_SIMULATOR: u64 = 3;
/// Below this much free memory, no new simulator step should start.
pub const MIN_FREE_MEMORY_PERCENT: u32 = 15;
/// Agent calls at once per vendor. A subscription's usage limit bounds this,
/// not the machine.
pub const AGENT_CALLS_PER_VENDOR: u32 = 2;
/// Disk a run needs room for: a worktree, a build folder, simulator data.
pub const MIN_FREE_DISK_GB: u64 = 20;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Machine {
    pub chip: String,
    pub cores: u32,
    /// Fast cores. The same as `cores` where the machine has one kind.
    pub performance_cores: u32,
    pub memory_gb: u64,
    /// `None` when the system would not say.
    pub memory_free_percent: Option<u32>,
    /// Load average over 1, 5 and 15 minutes. Shown, not used: booted
    /// simulators raise it far above what the CPU is doing.
    pub load: [f64; 3],
    /// The share of all cores doing nothing, sampled over a second.
    pub cpu_idle_percent: Option<u32>,
    pub disk_free_gb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Tool {
    pub name: String,
    pub ok: bool,
    /// The version, or what is wrong.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Simulators {
    /// iPhone simulators on the newest installed iOS runtime.
    pub iphones: u32,
    pub runtime: String,
    pub booted: u32,
}

/// How many steps of each kind to allow at once, and why.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Proposal {
    pub simulator_steps: u32,
    pub agent_calls_per_vendor: u32,
    pub reasons: Vec<String>,
}

/// How many more simulator steps could start right now.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Room {
    pub simulator_steps: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub machine: Machine,
    pub tools: Vec<Tool>,
    pub simulators: Option<Simulators>,
    pub proposal: Proposal,
    pub room_now: Room,
    /// The pools this machine's config sets. Empty means one of everything.
    pub pools: BTreeMap<String, u32>,
    pub warnings: Vec<String>,
}

/// What the machine's size allows, whatever it is doing now.
pub fn propose(machine: &Machine) -> Proposal {
    let by_cores = (machine.cores as f64 / CORES_PER_SIMULATOR_STEP).floor() as u32;
    let by_memory = ((machine.memory_gb as f64 - GB_RESERVED) / GB_PER_SIMULATOR_STEP).floor().max(0.0) as u32;
    let wanted = by_cores.min(by_memory);
    let simulator_steps = wanted.clamp(1, MAX_SIMULATOR_STEPS);
    let mut reasons = vec![
        format!("{} cores allow {by_cores} simulator step(s), at {CORES_PER_SIMULATOR_STEP} cores each", machine.cores),
        format!("{} GB of memory allow {by_memory}, at {GB_PER_SIMULATOR_STEP} GB each with {GB_RESERVED} GB kept back", machine.memory_gb),
    ];
    if wanted > MAX_SIMULATOR_STEPS {
        reasons.push(format!("capped at {MAX_SIMULATOR_STEPS}: more than that at once has not been measured"));
    }
    if machine.disk_free_gb.is_some() {
        reasons.push(format!("each simulator kept for this takes about {GB_DISK_PER_SIMULATOR} GB of disk"));
    }
    if wanted == 0 {
        reasons.push("the machine is below what one simulator step is assumed to need, so 1 is the floor and it may be slow".to_string());
    }
    reasons.push(format!("{AGENT_CALLS_PER_VENDOR} agent calls per vendor: a subscription's usage limit bounds this, not the machine"));
    Proposal { simulator_steps, agent_calls_per_vendor: AGENT_CALLS_PER_VENDOR, reasons }
}

/// What the machine's load allows this minute, never more than the proposal.
pub fn room_now(machine: &Machine, proposal: &Proposal) -> Room {
    if let Some(free) = machine.memory_free_percent {
        if free < MIN_FREE_MEMORY_PERCENT {
            return Room { simulator_steps: 0, reason: format!("only {free}% of memory is free, and a new simulator step waits below {MIN_FREE_MEMORY_PERCENT}%") };
        }
    }
    let Some(idle_percent) = machine.cpu_idle_percent else {
        return Room { simulator_steps: proposal.simulator_steps, reason: "the CPU's idle share is unknown, so the whole allowance is assumed free".to_string() };
    };
    // What is busy now is taken off the allowance, a step's worth at a time.
    // The first busy core is ignored: an idle machine is never quite idle.
    let busy_cores = machine.cores as f64 * (100 - idle_percent.min(100)) as f64 / 100.0;
    let taken = ((busy_cores - BUSY_CORES_IGNORED) / CORES_PER_SIMULATOR_STEP).ceil().max(0.0) as u32;
    let simulator_steps = proposal.simulator_steps.saturating_sub(taken);
    let reason = if taken == 0 {
        format!("the CPU is {idle_percent}% idle, so the whole allowance is free")
    } else {
        format!("the CPU is {idle_percent}% idle: about {busy_cores:.0} of {} cores are busy, which takes up {taken} of the {} allowed", machine.cores, proposal.simulator_steps)
    };
    Room { simulator_steps, reason }
}

/// Runs a command and returns its output, stdout then stderr, when it ran.
fn output(program: &str, args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new(program).args(args).output().ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text.trim().to_string()))
}

fn sysctl(name: &str) -> Option<String> {
    output("sysctl", &["-n", name]).filter(|(ok, _)| *ok).map(|(_, text)| text)
}

fn first_line(text: &str) -> String {
    text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// The first whole number in a text.
fn number(text: &str) -> Option<u64> {
    let digits: String = text.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The idle share in a line such as `CPU usage: 19.94% user, 9.60% sys, 70.45% idle`.
pub fn parse_idle(line: &str) -> Option<u32> {
    let before = line.split("% idle").next()?;
    let number = before.rsplit([' ', ',']).next()?;
    number.parse::<f64>().ok().map(|idle| idle.round() as u32)
}

pub fn parse_load(text: &str) -> [f64; 3] {
    let found: Vec<f64> = text.split(|c: char| !c.is_ascii_digit() && c != '.').filter_map(|part| part.parse().ok()).collect();
    [found.first().copied().unwrap_or(0.0), found.get(1).copied().unwrap_or(0.0), found.get(2).copied().unwrap_or(0.0)]
}

fn machine() -> Machine {
    let cores = sysctl("hw.physicalcpu").and_then(|t| t.parse().ok()).unwrap_or(1);
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    Machine {
        chip: sysctl("machdep.cpu.brand_string").unwrap_or_else(|| "unknown".to_string()),
        cores,
        performance_cores: sysctl("hw.perflevel0.physicalcpu").and_then(|t| t.parse().ok()).unwrap_or(cores),
        memory_gb: sysctl("hw.memsize").and_then(|t| t.parse::<u64>().ok()).map(|bytes| bytes / 1_073_741_824).unwrap_or(0),
        memory_free_percent: output("memory_pressure", &[]).and_then(|(_, text)| text.lines().rev().find(|l| l.contains("free percentage")).and_then(number)).map(|n| n as u32),
        load: sysctl("vm.loadavg").map(|t| parse_load(&t)).unwrap_or([0.0; 3]),
        // The first sample of `top` covers the time since boot. The second covers the last second.
        cpu_idle_percent: output("top", &["-l", "2", "-n", "0", "-s", "1"]).and_then(|(_, text)| text.lines().rev().find(|l| l.starts_with("CPU usage")).and_then(parse_idle)),
        // Kilobyte blocks, fourth column of the last line.
        disk_free_gb: output("df", &["-k", &home]).and_then(|(_, text)| text.lines().last().and_then(|l| l.split_whitespace().nth(3)).and_then(|kb| kb.parse::<u64>().ok())).map(|kb| kb / 1_048_576),
    }
}

fn tool(name: &str, program: &str, args: &[&str]) -> Tool {
    match output(program, args) {
        Some((true, text)) => Tool { name: name.to_string(), ok: true, detail: first_line(&text) },
        Some((false, text)) => Tool { name: name.to_string(), ok: false, detail: format!("it answered with an error: {}", first_line(&text)) },
        None => Tool { name: name.to_string(), ok: false, detail: "not installed, or not on the PATH".to_string() },
    }
}

/// The Command Line Tools must be of the same major version as macOS, or
/// Homebrew refuses to install anything.
fn command_line_tools() -> Tool {
    let name = "Command Line Tools".to_string();
    let system = output("sw_vers", &["-productVersion"]).map(|(_, t)| t).unwrap_or_default();
    let Some((true, info)) = output("pkgutil", &["--pkg-info=com.apple.pkg.CLTools_Executables"]) else {
        return Tool { name, ok: false, detail: "not installed".to_string() };
    };
    let version = info.lines().find_map(|l| l.strip_prefix("version: ")).unwrap_or("").to_string();
    let major = |v: &str| v.split('.').next().unwrap_or("").to_string();
    if major(&version) == major(&system) {
        Tool { name, ok: true, detail: format!("{version}, matching macOS {system}") }
    } else {
        Tool { name, ok: false, detail: format!("{version}, but macOS is {system}: update them, or Homebrew will refuse to install") }
    }
}

/// The iOS version in a runtime's key. `SimRuntime.iOS-27-2` and the
/// device-specific `SimRuntime.iOS-27-2-iPhone19-4` are both iOS 27.2.
pub fn runtime_version(key: &str) -> Option<Vec<u64>> {
    let rest = key.split("SimRuntime.iOS-").nth(1)?;
    let version: Vec<u64> = rest.split('-').map_while(|part| part.parse().ok()).collect();
    (!version.is_empty()).then_some(version)
}

/// Counts the iPhone simulators on the newest iOS version in `simctl`'s
/// JSON, across every runtime of that version.
pub fn count_simulators(json: &str) -> Option<Simulators> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let devices = value.get("devices")?.as_object()?;
    let newest = devices.keys().filter_map(|key| runtime_version(key)).max()?;
    let lists = || devices.iter().filter_map(|(key, list)| Some((runtime_version(key), list.as_array()?)));
    let is_phone = |d: &serde_json::Value| d.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.contains("iPhone") || n.starts_with("factory-sim"));
    let is_booted = |d: &&serde_json::Value| d.get("state").and_then(|s| s.as_str()) == Some("Booted");
    Some(Simulators {
        iphones: lists().filter(|(version, _)| version.as_ref() == Some(&newest)).flat_map(|(_, list)| list.iter()).filter(|d| is_phone(d)).count() as u32,
        runtime: newest.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("."),
        booted: lists().flat_map(|(_, list)| list.iter()).filter(is_booted).count() as u32,
    })
}

fn simulators() -> Option<Simulators> {
    let (ok, text) = output("xcrun", &["simctl", "list", "devices", "available", "-j"])?;
    if !ok {
        return None;
    }
    count_simulators(&text)
}

/// The pool sizes a proposal stands for, as the machine config holds them.
/// Agent calls are capped per subscription: the providers this machine
/// lists, or the two programs a workflow can name when it lists none.
pub fn pools_of(proposal: &Proposal, providers: &[String]) -> BTreeMap<String, u32> {
    let mut pools = BTreeMap::from([("simulator".to_string(), proposal.simulator_steps)]);
    let named: Vec<String> = if providers.is_empty() { vec!["codex".into(), "claude".into()] } else { providers.to_vec() };
    for name in named {
        pools.insert(name, proposal.agent_calls_per_vendor);
    }
    pools
}

/// The name of the simulator that belongs to a slot. A repository's
/// `verify.sh` looks for this name.
pub fn simulator_name(slot: u32) -> String {
    format!("factory-sim-{slot}")
}

/// `simctl`'s answer as JSON.
fn simctl_json(what: &str) -> Result<serde_json::Value> {
    let (ok, text) = output("xcrun", &["simctl", "list", what, "-j"]).ok_or_else(|| anyhow!("xcrun is not available"))?;
    if !ok {
        bail!("simctl could not list {what}: {}", first_line(&text));
    }
    serde_json::from_str(&text).map_err(|e| anyhow!("simctl's list of {what} is not JSON: {e}"))
}

/// The runtime to make a simulator on: the newest iOS, and of two runtimes
/// with that version the plainly named one.
pub fn pick_runtime(keys: &[String]) -> Option<String> {
    let newest = keys.iter().filter_map(|key| runtime_version(key)).max()?;
    keys.iter().filter(|key| runtime_version(key).as_ref() == Some(&newest)).min_by_key(|key| key.len()).cloned()
}

/// Makes sure the simulators for `count` slots exist and have been used
/// once. A simulator's first boot indexes and migrates for minutes, which
/// must not happen in the middle of a run. Returns what it did, per slot.
pub fn prepare_simulators(count: u32) -> Result<Vec<String>> {
    let devices = simctl_json("devices")?;
    let lists = devices.get("devices").and_then(|d| d.as_object()).ok_or_else(|| anyhow!("simctl listed no devices"))?;
    let existing = |name: &str| -> Option<String> {
        lists.values().filter_map(|list| list.as_array()).flatten().find(|d| d.get("name").and_then(|n| n.as_str()) == Some(name)).and_then(|d| d.get("udid")?.as_str().map(str::to_string))
    };
    let keys: Vec<String> = lists.keys().cloned().collect();
    let runtime = pick_runtime(&keys).ok_or_else(|| anyhow!("no iOS simulator runtime is installed"))?;
    let types = simctl_json("devicetypes")?;
    let type_list = types.get("devicetypes").and_then(|d| d.as_array()).cloned().unwrap_or_default();
    let named = |wanted: &str| type_list.iter().find(|t| t.get("name").and_then(|n| n.as_str()) == Some(wanted));
    // A plain current iPhone. Any iPhone will do when that model is not there.
    let device_type = named("iPhone 17")
        .or_else(|| type_list.iter().rev().find(|t| t.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.starts_with("iPhone"))))
        .and_then(|t| t.get("identifier")?.as_str().map(str::to_string))
        .ok_or_else(|| anyhow!("no iPhone device type is installed"))?;

    let mut done = Vec::new();
    for slot in 1..=count {
        let name = simulator_name(slot);
        if existing(&name).is_some() {
            println!("  {name}: already there");
            done.push(format!("{name}: already there"));
            continue;
        }
        println!("  {name}: creating, then using it once. This takes a few minutes and keeps the machine busy");
        // How idle the machine is before the boot. "Calm again" is measured against this.
        let before = idle_now();
        let (ok, udid) = output("xcrun", &["simctl", "create", &name, &device_type, &runtime]).ok_or_else(|| anyhow!("xcrun is not available"))?;
        if !ok {
            bail!("could not create {name}: {}", first_line(&udid));
        }
        let udid = first_line(&udid);
        let _ = output("xcrun", &["simctl", "boot", &udid]);
        let _ = output("xcrun", &["simctl", "bootstatus", &udid]);
        // Maestro installs its driver on first contact. Do that now too.
        let _ = Command::new("maestro").args(["--device", &udid, "hierarchy"]).env("MAESTRO_CLI_NO_ANALYTICS", "1").output();
        // Its first boot is over when the machine has calmed down: three
        // readings in a row near what it was before, and not in the first minute.
        let started = std::time::Instant::now();
        let limit = std::time::Duration::from_secs(FIRST_BOOT_WAIT_SECONDS);
        let mut calm = 0;
        while calm < 3 && started.elapsed() < limit {
            let idle = idle_now();
            let early = started.elapsed() < std::time::Duration::from_secs(60);
            calm = if !early && calm_again(before, idle) { calm + 1 } else { 0 };
        }
        let _ = output("xcrun", &["simctl", "shutdown", &udid]);
        if calm >= 3 {
            println!("  {name}: ready after {} s", started.elapsed().as_secs());
        } else {
            println!("  {name}: the machine was still busy after {} s. Shut down anyway, its next boot may be slow", started.elapsed().as_secs());
        }
        done.push(format!("{name}: created"));
    }
    Ok(done)
}

/// The longest `prepare_simulators` waits for one simulator's first boot to settle.
const FIRST_BOOT_WAIT_SECONDS: u64 = 600;

/// The CPU's idle share over the next five seconds, in percent.
fn idle_now() -> Option<u32> {
    output("top", &["-l", "2", "-n", "0", "-s", "5"]).and_then(|(_, text)| text.lines().rev().find(|l| l.starts_with("CPU usage")).and_then(parse_idle))
}

/// Whether the machine is about as idle as it was before. A machine that
/// was busy to begin with is never asked to be idler than it was.
fn calm_again(before: Option<u32>, now: Option<u32>) -> bool {
    let wanted = before.map_or(70, |b| b.saturating_sub(10).min(70));
    now.is_some_and(|idle| idle >= wanted)
}

/// Reads the machine. Takes a few seconds: several tools are asked for their version.
pub fn read() -> Report {
    let machine = machine();
    let tools = vec![
        tool("Xcode", "xcodebuild", &["-version"]),
        command_line_tools(),
        tool("Maestro", "maestro", &["--version"]),
        tool("Java", "java", &["-version"]),
        tool("git", "git", &["--version"]),
        tool("gh", "gh", &["--version"]),
        tool("gh login", "gh", &["auth", "token"]).redact(),
        tool("codex", "codex", &["--version"]),
        tool("claude", "claude", &["--version"]),
    ];
    let simulators = simulators();
    let proposal = propose(&machine);
    let room_now = room_now(&machine, &proposal);
    let mut warnings: Vec<String> = tools.iter().filter(|t| !t.ok).map(|t| format!("{}: {}", t.name, t.detail)).collect();
    if machine.disk_free_gb.is_some_and(|gb| gb < MIN_FREE_DISK_GB) {
        warnings.push(format!("only {} GB of disk is free, and a run wants {MIN_FREE_DISK_GB}", machine.disk_free_gb.unwrap_or(0)));
    }
    match &simulators {
        None => warnings.push("no simulator list: is Xcode installed and selected?".to_string()),
        Some(found) if found.iphones < proposal.simulator_steps => warnings.push(format!(
            "{} iPhone simulator(s) on iOS {}, fewer than the {} steps proposed",
            found.iphones, found.runtime, proposal.simulator_steps
        )),
        Some(_) => {}
    }
    let pools = super::util::home().ok().and_then(|home| super::config::Config::load(&home).ok()).map(|c| c.pools).unwrap_or_default();
    Report { machine, tools, simulators, proposal, room_now, pools, warnings }
}

impl Tool {
    /// For a check whose output is a secret: keep only whether it worked.
    fn redact(mut self) -> Tool {
        self.detail = if self.ok { "logged in".to_string() } else { "not logged in: run `gh auth login`".to_string() };
        self
    }
}

pub fn print(report: &Report) {
    let m = &report.machine;
    println!("machine");
    println!("  {}: {} cores ({} performance), {} GB of memory", m.chip, m.cores, m.performance_cores, m.memory_gb);
    let free = m.memory_free_percent.map(|p| format!("{p}% of memory free")).unwrap_or_else(|| "memory use unknown".to_string());
    let disk = m.disk_free_gb.map(|gb| format!("{gb} GB of disk free")).unwrap_or_else(|| "disk unknown".to_string());
    let idle = m.cpu_idle_percent.map(|p| format!("CPU {p}% idle")).unwrap_or_else(|| "CPU use unknown".to_string());
    println!("  now: {idle}, {free}, {disk}");
    println!("  load average {:.1} / {:.1} / {:.1} over 1, 5 and 15 minutes (booted simulators inflate it)", m.load[0], m.load[1], m.load[2]);
    println!("tools");
    for tool in &report.tools {
        println!("  {} {}: {}", if tool.ok { "ok  " } else { "FAIL" }, tool.name, tool.detail);
    }
    match &report.simulators {
        Some(s) => println!("simulators\n  {} iPhone simulator(s) on iOS {}, {} booted now", s.iphones, s.runtime, s.booted),
        None => println!("simulators\n  none found"),
    }
    println!("what this machine can run at once");
    println!("  {} step(s) that build and use a simulator", report.proposal.simulator_steps);
    println!("  {} agent call(s) per vendor", report.proposal.agent_calls_per_vendor);
    for reason in &report.proposal.reasons {
        println!("    - {reason}");
    }
    println!("right now");
    println!("  room for {} more simulator step(s): {}", report.room_now.simulator_steps, report.room_now.reason);
    if !report.warnings.is_empty() {
        println!("needs attention");
        for warning in &report.warnings {
            println!("  - {warning}");
        }
    }
    println!("in use on this machine");
    if report.pools.is_empty() {
        println!("  no pools are set: one simulator step at a time, and no cap on agent calls");
        println!("  `factory capacity --write` sets the proposal above. `--prepare` makes the simulators for it");
    } else {
        let set: Vec<String> = report.pools.iter().map(|(name, size)| format!("{name} {size}")).collect();
        println!("  {}", set.join(", "));
    }
    println!("the proposal is sized from the trial app, which builds in seconds. A long build needs more cores per step");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(cores: u32, performance: u32, memory_gb: u64) -> Machine {
        Machine {
            chip: "test".into(),
            cores,
            performance_cores: performance,
            memory_gb,
            memory_free_percent: Some(70),
            load: [1.0, 1.0, 1.0],
            cpu_idle_percent: Some(95),
            disk_free_gb: Some(300),
        }
    }

    #[test]
    fn the_smaller_of_cores_and_memory_decides() {
        // A laptop with little memory is held by memory.
        let small = propose(&machine(8, 4, 8));
        assert_eq!(small.simulator_steps, 1);
        assert!(small.reasons.iter().any(|r| r.contains("below what one simulator step")), "{:?}", small.reasons);
        // 16 GB allows one by memory, though its cores would allow two.
        assert_eq!(propose(&machine(8, 4, 16)).simulator_steps, 1);
        assert_eq!(propose(&machine(8, 4, 32)).simulator_steps, 2);
        // The machine the numbers were measured on: three at once.
        let measured = propose(&machine(12, 8, 96));
        assert_eq!(measured.simulator_steps, 3);
        assert!(measured.reasons[0].contains("12 cores allow 3"), "{:?}", measured.reasons);
        assert!(measured.reasons[1].contains("96 GB of memory allow 11"), "{:?}", measured.reasons);
        // A larger machine is capped at what has been measured.
        let studio = propose(&machine(24, 16, 128));
        assert_eq!(studio.simulator_steps, MAX_SIMULATOR_STEPS);
        assert!(studio.reasons.iter().any(|r| r.contains("capped at")), "{:?}", studio.reasons);
        assert_eq!(studio.agent_calls_per_vendor, AGENT_CALLS_PER_VENDOR);
    }

    #[test]
    fn a_busy_machine_has_less_room_than_its_size_allows() {
        let mut m = machine(12, 8, 96);
        let proposal = propose(&m);
        assert_eq!(room_now(&m, &proposal).simulator_steps, 3);
        // Someone is compiling: a fifth of the CPU is left.
        m.cpu_idle_percent = Some(20);
        let room = room_now(&m, &proposal);
        assert_eq!(room.simulator_steps, 0);
        assert!(room.reason.contains("20% idle: about 10 of 12 cores are busy, which takes up 3 of the 3 allowed"), "{}", room.reason);
        // Half busy leaves one.
        m.cpu_idle_percent = Some(50);
        assert_eq!(room_now(&m, &proposal).simulator_steps, 1);
        // Three idle simulators are booted. The load average is high, the CPU is not busy.
        m.cpu_idle_percent = Some(70);
        m.load = [7.2, 25.0, 49.8];
        assert_eq!(room_now(&m, &proposal).simulator_steps, 2);
        // Memory is nearly gone, whatever the CPU says.
        m.memory_free_percent = Some(9);
        let room = room_now(&m, &proposal);
        assert_eq!(room.simulator_steps, 0);
        assert!(room.reason.contains("only 9% of memory is free"), "{}", room.reason);
        // What the system would not say does not block.
        m.memory_free_percent = None;
        m.cpu_idle_percent = None;
        assert_eq!(room_now(&m, &proposal).simulator_steps, 3);
    }

    #[test]
    fn simulators_are_counted_on_the_newest_ios_whatever_the_runtime_is_called() {
        let json = r#"{"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-26-5":[{"name":"iPhone 17","state":"Booted"},{"name":"iPhone 17 Pro","state":"Shutdown"}],
            "com.apple.CoreSimulator.SimRuntime.iOS-27-2":[{"name":"iPhone 17","state":"Shutdown"},{"name":"iPad mini","state":"Shutdown"},{"name":"factory-sim-1","state":"Booted"}],
            "com.apple.CoreSimulator.SimRuntime.iOS-27-2-iPhone19-4":[{"name":"iPhone Duo","state":"Shutdown"}],
            "com.apple.CoreSimulator.SimRuntime.tvOS-27-2":[{"name":"Apple TV","state":"Shutdown"}]}}"#;
        let found = count_simulators(json).unwrap();
        // Two runtimes are iOS 27.2. iPads and other platforms don't count.
        assert_eq!(found, Simulators { iphones: 3, runtime: "27.2".into(), booted: 2 });
        assert_eq!(runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-27-2-iPhone19-4"), Some(vec![27, 2]));
        assert_eq!(runtime_version("com.apple.CoreSimulator.SimRuntime.tvOS-27-2"), None);
        assert!(count_simulators("{}").is_none());
    }

    #[test]
    fn a_proposal_becomes_pools_and_slots_have_simulators() {
        let pools = pools_of(&propose(&machine(12, 8, 96)), &[]);
        assert_eq!(pools.get("simulator"), Some(&3));
        assert_eq!((pools.get("codex"), pools.get("claude")), (Some(&2), Some(&2)));
        // A machine that lists its subscriptions gets a cap for each of those.
        let listed = pools_of(&propose(&machine(12, 8, 96)), &["chatgpt".to_string(), "glm".to_string()]);
        assert_eq!(listed.keys().map(String::as_str).collect::<Vec<_>>(), ["chatgpt", "glm", "simulator"]);
        assert_eq!(simulator_name(2), "factory-sim-2");
        // Of two runtimes with the newest version, the plainly named one.
        let keys: Vec<String> = ["com.apple.CoreSimulator.SimRuntime.iOS-26-5", "com.apple.CoreSimulator.SimRuntime.iOS-27-2-iPhone19-4", "com.apple.CoreSimulator.SimRuntime.iOS-27-2", "com.apple.CoreSimulator.SimRuntime.tvOS-27-2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pick_runtime(&keys).as_deref(), Some("com.apple.CoreSimulator.SimRuntime.iOS-27-2"));
        assert_eq!(pick_runtime(&["com.apple.CoreSimulator.SimRuntime.tvOS-27-2".to_string()]), None);
    }

    #[test]
    fn calm_is_judged_against_how_idle_the_machine_was() {
        // An idle machine must come back to 70%.
        assert!(calm_again(Some(95), Some(72)) && !calm_again(Some(95), Some(60)));
        // One that idled at 66% is calm again at 56%, not before.
        assert!(calm_again(Some(66), Some(58)) && !calm_again(Some(66), Some(40)));
        // No reading is not calm. No reading before means the plain 70%.
        assert!(!calm_again(Some(66), None) && calm_again(None, Some(70)) && !calm_again(None, Some(69)));
    }

    #[test]
    fn system_answers_are_parsed() {
        assert_eq!(parse_load("{ 2.47 3.08 3.54 }"), [2.47, 3.08, 3.54]);
        assert_eq!(parse_load(""), [0.0, 0.0, 0.0]);
        assert_eq!(parse_idle("CPU usage: 19.94% user, 9.60% sys, 70.45% idle "), Some(70));
        assert_eq!(parse_idle("CPU usage: 3.1% user, 2.0% sys, 94.9% idle"), Some(95));
        assert_eq!(parse_idle("Processes: 812 total"), None);
        assert_eq!(number("System-wide memory free percentage: 71%"), Some(71));
        assert_eq!(number("no digits"), None);
        assert_eq!(first_line("\n\nXcode 27.2\nBuild version 1"), "Xcode 27.2");
    }
}
