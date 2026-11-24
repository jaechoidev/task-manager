//! Runs one child process with a time limit.
//!
//! Each child gets its own process group, so a hung agent is killed together
//! with everything it started.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Spec<'a> {
    pub program: &'a str,
    pub args: Vec<String>,
    pub cwd: &'a Path,
    pub stdin: Option<&'a str>,
    pub stdout: &'a Path,
    pub stderr: &'a Path,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    /// `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration: Duration,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

fn signal_group(pgid: u32, signal: &str) {
    // `kill -TERM -- -<pgid>` signals every process in the group.
    let _ = Command::new("/bin/kill").args([signal, "--", &format!("-{pgid}")]).status();
}

pub fn run(spec: &Spec) -> Result<Outcome> {
    let stdout = File::create(spec.stdout).with_context(|| format!("creating {}", spec.stdout.display()))?;
    let stderr = File::create(spec.stderr).with_context(|| format!("creating {}", spec.stderr.display()))?;

    let mut command = Command::new(spec.program);
    command
        .args(&spec.args)
        .current_dir(spec.cwd)
        .stdin(if spec.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0);

    let started = Instant::now();
    let mut child = command.spawn().with_context(|| format!("starting {}", spec.program))?;
    let pgid = child.id();

    // Written from a thread so a child that reads slowly can't block the timer.
    let writer = spec.stdin.map(|text| {
        let text = text.to_string();
        let mut pipe = child.stdin.take();
        std::thread::spawn(move || {
            if let Some(pipe) = pipe.as_mut() {
                let _ = pipe.write_all(text.as_bytes());
            }
        })
    });

    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= spec.timeout {
            timed_out = true;
            signal_group(pgid, "-TERM");
            let grace = Instant::now();
            while child.try_wait()?.is_none() && grace.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(100));
            }
            signal_group(pgid, "-KILL");
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }

    Ok(Outcome { exit_code: status.code(), timed_out, duration: started.elapsed() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(dir: &'a Path, args: &[&str], stdin: Option<&'a str>, out: &'a Path, err: &'a Path, secs: u64) -> Spec<'a> {
        Spec {
            program: "/bin/sh",
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: dir,
            stdin,
            stdout: out,
            stderr: err,
            timeout: Duration::from_secs(secs),
        }
    }

    #[test]
    fn captures_output_and_exit_code() {
        let dir = std::env::temp_dir();
        let out = dir.join(format!("factory-proc-{}-a.out", std::process::id()));
        let err = dir.join(format!("factory-proc-{}-a.err", std::process::id()));
        let result = run(&spec(&dir, &["-c", "cat; exit 3"], Some("hello"), &out, &err, 10)).unwrap();
        assert_eq!(result.exit_code, Some(3));
        assert!(!result.timed_out && !result.ok());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello");
    }

    #[test]
    fn kills_the_whole_group_on_timeout() {
        let dir = std::env::temp_dir();
        let out = dir.join(format!("factory-proc-{}-b.out", std::process::id()));
        let err = dir.join(format!("factory-proc-{}-b.err", std::process::id()));
        // The shell starts a child of its own. Both must be gone afterwards.
        let result = run(&spec(&dir, &["-c", "sleep 60 & echo $!; wait"], None, &out, &err, 1)).unwrap();
        assert!(result.timed_out);
        assert!(result.duration < Duration::from_secs(10));
        let grandchild = std::fs::read_to_string(&out).unwrap().trim().to_string();
        std::thread::sleep(Duration::from_millis(300));
        let alive = Command::new("/bin/kill").args(["-0", &grandchild]).stderr(Stdio::null()).status().unwrap();
        assert!(!alive.success(), "process {grandchild} outlived its group");
    }
}
