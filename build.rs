//! Puts the commit the program is built from into `FACTORY_COMMIT`, so
//! `factory --version` and `/v1/health` say which build runs. A build with
//! uncommitted changes to tracked files ends in `-dirty`. Built without git,
//! as from a source archive, it is `unknown`.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !text.is_empty()).then_some(text)
}

fn main() {
    let commit = match git(&["rev-parse", "--short=12", "HEAD"]) {
        Some(commit) if git(&["status", "--porcelain", "--untracked-files=no"]).is_some() => format!("{commit}-dirty"),
        Some(commit) => commit,
        None => "unknown".to_string(),
    };
    println!("cargo:rustc-env=FACTORY_COMMIT={commit}");
    // Built again when the commit moves, or when a tracked file changes.
    let mut watched = vec!["HEAD".to_string(), "packed-refs".to_string(), "index".to_string()];
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(reference);
    }
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    for path in ["src", "skills", "tests", "Cargo.toml", "Cargo.lock", "build.rs"] {
        println!("cargo:rerun-if-changed={path}");
    }
}
