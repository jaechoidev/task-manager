//! One headless agent call: a prompt in, schema-checked JSON out.
//!
//! Agents are confined. A read-only call can't change files. A
//! workspace-write call can only write inside its worktree. Neither is given
//! git or network commands: the engine commits, pushes and opens the PR.
//!
//! How each vendor is confined:
//! - Codex runs in its own OS sandbox, which blocks writes outside the
//!   worktree. It can still read files anywhere on the machine.
//! - Claude has no OS sandbox. Two settings make its file tools refuse reads
//!   and writes outside the worktree, and it gets no shell beyond a syntax
//!   check. Listing `Write` as an allowed tool is not enough by itself: that
//!   allows writes anywhere.

use super::proc::{self, Outcome};
use super::workflow::{Access, Vendor};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Call<'a> {
    pub vendor: Vendor,
    pub model: Option<&'a str>,
    pub access: Access,
    /// The worktree the agent runs in.
    pub cwd: &'a Path,
    pub prompt: &'a str,
    /// JSON Schema text for the final message.
    pub schema: &'a str,
    /// Where this call's files go.
    pub dir: &'a Path,
    /// File name prefix, for example `01-develop-1`.
    pub stem: &'a str,
    pub timeout: Duration,
    /// A session of the same vendor to continue, so the agent keeps what it
    /// learned in earlier rounds.
    pub resume: Option<&'a str>,
}

/// Settings every Claude call runs with, whatever its tools.
const CLAUDE_SETTINGS: &str =
    r#"{"permissions":{"blockWritesOutsideWorkingDirectories":true,"blockReadsOutsideWorkingDirectories":true}}"#;

pub struct Reply {
    pub outcome: Outcome,
    /// The agent's final JSON, when it produced any.
    pub output: Option<Value>,
    /// Why there is no usable output.
    pub error: Option<String>,
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub session_id: Option<String>,
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

pub fn call(call: &Call) -> Result<Reply> {
    let file = |suffix: &str| call.dir.join(format!("{}.{suffix}", call.stem));
    let input_path = file("prompt.md");
    let output_path = file("out.json");
    let stdout_path = file("stdout.log");
    let stderr_path = file("stderr.log");
    let schema_path = file("schema.json");
    std::fs::write(&input_path, call.prompt).with_context(|| format!("writing {}", input_path.display()))?;
    std::fs::write(&schema_path, call.schema)?;
    let _ = std::fs::remove_file(&output_path);

    let (program, args) = match call.vendor {
        Vendor::Codex => ("codex", codex_args(call, &schema_path, &output_path)),
        Vendor::Claude => ("claude", claude_args(call)),
    };
    let outcome = proc::run(&proc::Spec {
        program,
        args,
        cwd: call.cwd,
        stdin: Some(call.prompt),
        stdout: &stdout_path,
        stderr: &stderr_path,
        timeout: call.timeout,
    })?;

    let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
    let (output, session_id, mut error) = match call.vendor {
        Vendor::Codex => read_codex(&stdout, &output_path),
        Vendor::Claude => read_claude(&stdout),
    };
    if let (Vendor::Claude, Some(value)) = (call.vendor, &output) {
        std::fs::write(&output_path, serde_json::to_string_pretty(value)?)?;
    }
    if outcome.timed_out {
        error = Some(format!("timed out after {} minutes", call.timeout.as_secs() / 60));
    } else if !outcome.ok() && error.is_none() {
        error = Some(format!("{program} exited with {:?}", outcome.exit_code));
    }

    Ok(Reply { outcome, output, error, input_path, output_path, stdout_path, stderr_path, session_id })
}

fn codex_args(call: &Call, schema: &Path, output: &Path) -> Vec<String> {
    let sandbox = match call.access {
        Access::ReadOnly => "read-only",
        Access::WorkspaceWrite => "workspace-write",
    };
    let mut args: Vec<String> = match call.resume {
        None => ["exec", "--cd", &text(call.cwd), "--sandbox", sandbox, "--color", "never"].iter().map(|s| s.to_string()).collect(),
        // `exec resume` has no --sandbox, --cd or --color. The sandbox comes
        // from a config override and the directory from the process.
        Some(session) => vec!["exec".into(), "resume".into(), session.into(), "-c".into(), format!("sandbox_mode=\"{sandbox}\"")],
    };
    args.extend(["--output-schema".into(), text(schema), "-o".into(), text(output), "--json".into()]);
    if let Some(model) = call.model {
        args.extend(["-m".into(), model.into()]);
    }
    args.push("-".into()); // the prompt comes on stdin
    args
}

fn claude_args(call: &Call) -> Vec<String> {
    // In print mode a tool that is not allowed is refused, never asked about.
    // No shell for a reader: `git diff --output=FILE` alone would let it write
    // anywhere. A writer gets only Maestro's syntax check. It may not run
    // ./verify.sh, because it could edit that script first and so run anything.
    let (mode, tools): (&str, &[&str]) = match call.access {
        Access::ReadOnly => ("dontAsk", &["Read", "Glob", "Grep"]),
        Access::WorkspaceWrite => ("acceptEdits", &["Read", "Glob", "Grep", "Edit", "Write", "Bash(maestro check-syntax:*)"]),
    };
    let mut args: Vec<String> = ["-p", "--output-format", "json", "--json-schema", call.schema, "--settings", CLAUDE_SETTINGS]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(session) = call.resume {
        args.extend(["--resume".into(), session.into()]);
    }
    // Skip the user's MCP servers, plugins and skills. They add cost and are
    // not part of the task.
    args.extend(["--strict-mcp-config", "--setting-sources", "project,local", "--disable-slash-commands"].map(String::from));
    args.extend(["--permission-mode".into(), mode.into()]);
    if let Some(model) = call.model {
        args.extend(["--model".into(), model.into()]);
    }
    args.push("--allowedTools".into());
    args.extend(tools.iter().map(|t| t.to_string()));
    args
}

/// Codex writes its final message to a file and its events to stdout.
fn read_codex(stdout: &str, output: &Path) -> (Option<Value>, Option<String>, Option<String>) {
    let session = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|event| event.get("thread_id").and_then(Value::as_str).map(str::to_string));
    match std::fs::read_to_string(output) {
        Err(_) => (None, session, Some("codex wrote no final message".into())),
        Ok(raw) => match serde_json::from_str::<Value>(raw.trim()) {
            Ok(value) => (Some(value), session, None),
            Err(e) => (None, session, Some(format!("codex's final message is not JSON: {e}"))),
        },
    }
}

/// Claude prints one JSON result with the schema-checked reply inside it.
fn read_claude(stdout: &str) -> (Option<Value>, Option<String>, Option<String>) {
    let Ok(result) = serde_json::from_str::<Value>(stdout.trim()) else {
        return (None, None, Some("claude did not print a JSON result".into()));
    };
    let session = result.get("session_id").and_then(Value::as_str).map(str::to_string);
    if result.get("is_error").and_then(Value::as_bool) == Some(true) {
        let detail = result.get("result").and_then(Value::as_str).unwrap_or("no detail");
        return (None, session, Some(format!("claude reported an error: {detail}")));
    }
    match result.get("structured_output") {
        Some(value) if !value.is_null() => (Some(value.clone()), session, None),
        _ => (None, session, Some("claude returned no structured output".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_result_is_unwrapped() {
        let ok = r#"{"type":"result","is_error":false,"session_id":"s1","structured_output":{"verdict":"approve"}}"#;
        let (output, session, error) = read_claude(ok);
        assert_eq!(output.unwrap()["verdict"], "approve");
        assert_eq!(session.as_deref(), Some("s1"));
        assert!(error.is_none());

        let failed = r#"{"type":"result","is_error":true,"result":"401 token invalid"}"#;
        let (output, _, error) = read_claude(failed);
        assert!(output.is_none());
        assert!(error.unwrap().contains("401 token invalid"));
    }

    #[test]
    fn sandbox_flags_follow_access() {
        let dir = std::env::temp_dir();
        let call = Call {
            vendor: Vendor::Codex,
            model: None,
            access: Access::WorkspaceWrite,
            cwd: &dir,
            prompt: "p",
            schema: "{}",
            dir: &dir,
            stem: "s",
            timeout: Duration::from_secs(1),
            resume: None,
        };
        let codex = codex_args(&call, Path::new("schema.json"), Path::new("out.json")).join(" ");
        assert!(codex.contains("--sandbox workspace-write"), "{codex}");
        assert!(!codex.contains("danger"), "{codex}");

        // A resumed Codex call must name its sandbox again.
        let resumed = Call { resume: Some("abc-123"), ..call };
        let codex = codex_args(&resumed, Path::new("schema.json"), Path::new("out.json")).join(" ");
        assert!(codex.starts_with("exec resume abc-123 -c sandbox_mode=\"workspace-write\""), "{codex}");
        assert!(codex.ends_with("--json -"), "{codex}");
        assert!(!codex.contains("--cd") && !codex.contains("danger"), "{codex}");

        let review = Call { vendor: Vendor::Claude, access: Access::ReadOnly, resume: None, ..call };
        let claude = claude_args(&review).join(" ");
        assert!(claude.contains("--permission-mode dontAsk"), "{claude}");
        assert!(!claude.contains("Edit") && !claude.contains("Write ") && !claude.contains("Bash"), "{claude}");

        // Every Claude call is confined to its directory, and never gets a
        // shell that could run a script it edited.
        let writer = Call { vendor: Vendor::Claude, resume: Some("s-9"), ..call };
        let claude = claude_args(&writer).join(" ");
        for args in [&claude, &claude_args(&review).join(" ")] {
            assert!(args.contains("blockWritesOutsideWorkingDirectories\":true"), "{args}");
            assert!(args.contains("blockReadsOutsideWorkingDirectories\":true"), "{args}");
            assert!(!args.contains("verify.sh") && !args.contains("git "), "{args}");
        }
        assert!(claude.contains("--resume s-9") && claude.contains("--permission-mode acceptEdits"), "{claude}");
    }
}
