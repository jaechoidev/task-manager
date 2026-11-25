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
//! - Antigravity runs with its own `--sandbox`. That does not confine it:
//!   it wrote into an ordinary folder of the home directory when tried. The
//!   engine only uses a provider that passed `factory providers check`, and
//!   Antigravity does not. Its flags are here for that check.

use super::ledger;
use super::proc::{self, Outcome};
use super::providers::Cli;
use super::util;
use super::workflow::{Access, Vendor};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Who answers a call: the program, the model, and the login it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runner {
    pub cli: Cli,
    /// None leaves the choice to the program's own configuration.
    pub model: Option<String>,
    /// A shell file that selects the login, sourced before the program runs.
    pub env_file: Option<PathBuf>,
    /// A config folder that selects the login.
    pub config_dir: Option<PathBuf>,
}

impl Runner {
    /// A program with its default login, as a workflow step names it.
    pub fn of(vendor: Vendor, model: Option<&str>) -> Runner {
        let cli = match vendor {
            Vendor::Codex => Cli::Codex,
            Vendor::Claude => Cli::Claude,
        };
        Runner { cli, model: model.map(str::to_string), env_file: None, config_dir: None }
    }
}

pub struct Call<'a> {
    pub runner: &'a Runner,
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
    /// Whether the agent may search and read the web. Only Claude gets web
    /// tools this way. Codex runs without network either way.
    pub web: bool,
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
    /// How much of the provider's plan is used, when the call said.
    pub quota: Option<ledger::Reading>,
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

    let cli = call.runner.cli;
    let program = cli.program();
    let launch = launch(call, &schema_path, &output_path)?;
    let outcome = proc::run(&proc::Spec {
        program: &launch.program,
        args: launch.args,
        cwd: call.cwd,
        stdin: launch.stdin.as_deref(),
        stdout: &stdout_path,
        stderr: &stderr_path,
        timeout: call.timeout,
        env: launch.env,
        unset: launch.unset,
    })?;

    let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
    let (output, session_id, mut error) = match cli {
        Cli::Codex => read_codex(&stdout, &output_path),
        Cli::Claude => read_claude(&stdout),
        Cli::Agy => {
            let (output, error) = read_agy(&stdout, &std::fs::read_to_string(&stderr_path).unwrap_or_default());
            (output, None, error)
        }
    };
    // What the call said about the plan it ran on. Never a reason to fail.
    let now = util::now_secs();
    let quota = match cli {
        Cli::Codex => session_id.as_deref().and_then(|session| codex_limits(&codex_home(call.runner), session)).and_then(|limits| ledger::read_codex(&limits, now)),
        Cli::Claude => claude_limits(&stdout).and_then(|info| ledger::read_claude(&info, now)),
        Cli::Agy => None,
    };
    if let (Cli::Claude | Cli::Agy, Some(value)) = (cli, &output) {
        std::fs::write(&output_path, serde_json::to_string_pretty(value)?)?;
    }
    if outcome.timed_out {
        error = Some(format!("timed out after {} minutes", call.timeout.as_secs() / 60));
    } else if outcome.exit_code == Some(ENV_FILE_UNREADABLE) && call.runner.env_file.is_some() {
        error = Some("the env file could not be read".into());
    } else if !outcome.ok() {
        // What the program reported says more than its exit code.
        error = Some(match error {
            Some(reported) => format!("{program} exited with {:?}: {reported}", outcome.exit_code),
            None => format!("{program} exited with {:?}", outcome.exit_code),
        });
    }

    Ok(Reply { outcome, output, error, input_path, output_path, stdout_path, stderr_path, session_id, quota })
}

/// A command line, ready to run.
#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Variables the child must not inherit.
    pub unset: Vec<String>,
    pub stdin: Option<String>,
}

/// What the shell wrapper exits with when it can't read the env file.
const ENV_FILE_UNREADABLE: i32 = 97;

/// Runs a program with what an env file sets. `set -a` exports it. The file
/// is tested first, because a shell that can't source a file just exits.
pub(super) const WITH_ENV_FILE: &str = r#"[ -r "$0" ] || exit 97; set -a; . "$0"; set +a; exec "$@""#;

/// The inherited variables that could select a login for this program.
///
/// A runner with its own env file or config folder must start without them.
/// Found the hard way: inside a Claude Code session, `claude` ignores the key
/// an env file sets and sends the session's own credential, which then goes
/// to whatever endpoint the env file names.
pub fn login_variables(cli: Cli, names: impl Iterator<Item = String>) -> Vec<String> {
    let prefixes: &[&str] = match cli {
        Cli::Claude => &["ANTHROPIC_", "CLAUDE_", "CLAUDECODE"],
        Cli::Codex => &["OPENAI_", "CODEX_"],
        Cli::Agy => &["GEMINI_", "GOOGLE_", "ANTIGRAVITY_"],
    };
    let mut found: Vec<String> = names.filter(|name| prefixes.iter().any(|prefix| name.starts_with(prefix))).collect();
    found.sort();
    found
}

/// How a call is started: the program's own flags, and the runner's login.
pub fn launch(call: &Call, schema: &Path, output: &Path) -> Result<Launch> {
    let runner = call.runner;
    let (args, stdin) = match runner.cli {
        Cli::Codex => (codex_args(call, schema, output), Some(call.prompt.to_string())),
        Cli::Claude => (claude_args(call), Some(call.prompt.to_string())),
        Cli::Agy => (agy_args(call), None),
    };
    let program = runner.cli.program().to_string();
    // Its own login means none of ours.
    let unset = if runner.env_file.is_some() || runner.config_dir.is_some() {
        login_variables(runner.cli, std::env::vars_os().filter_map(|(name, _)| name.into_string().ok()))
    } else {
        Vec::new()
    };
    let mut env = Vec::new();
    if let Some(dir) = &runner.config_dir {
        let key = match runner.cli {
            Cli::Claude => "CLAUDE_CONFIG_DIR",
            Cli::Codex => "CODEX_HOME",
            Cli::Agy => bail!("config_dir is not supported for agy"),
        };
        env.push((key.to_string(), text(dir)));
    }
    match &runner.env_file {
        None => Ok(Launch { program, args, env, unset, stdin }),
        // The shell reads the file, so the key in it never passes through
        // the engine.
        Some(file) => {
            let mut wrapped = vec!["-c".to_string(), WITH_ENV_FILE.to_string(), text(file), program];
            wrapped.extend(args);
            Ok(Launch { program: "sh".into(), args: wrapped, env, unset, stdin })
        }
    }
}

/// How many tokens a call used, as the program itself counted them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Everything sent to the model, cached or not.
    pub input: u64,
    /// The part of the input that was read from a cache.
    pub cached: u64,
    pub output: u64,
}

/// The token counts in what a call printed. Claude gives them in its
/// result, with cached input counted apart from the rest. Codex gives them
/// once per turn, with cached input as a part of the input. Told apart by
/// shape, so a log can be read without knowing which program wrote it.
pub fn tokens(stdout: &str) -> Option<Tokens> {
    let number = |usage: &Value, key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let lines: Vec<Value> = stdout.lines().filter(|line| line.contains("\"usage\"")).filter_map(|line| serde_json::from_str(line).ok()).collect();
    if let Some(usage) = lines.iter().rev().find(|line| line.get("type").and_then(Value::as_str) == Some("result")).and_then(|result| result.get("usage")).filter(|usage| usage.is_object()) {
        let (fresh, written, read) = (number(usage, "input_tokens"), number(usage, "cache_creation_input_tokens"), number(usage, "cache_read_input_tokens"));
        return Some(Tokens { input: fresh + written + read, cached: read, output: number(usage, "output_tokens") });
    }
    let turns: Vec<&Value> = lines.iter().filter(|line| line.get("type").and_then(Value::as_str) == Some("turn.completed")).filter_map(|turn| turn.get("usage")).collect();
    if turns.is_empty() {
        return None;
    }
    Some(turns.iter().fold(Tokens::default(), |sum, usage| Tokens {
        input: sum.input + number(usage, "input_tokens"),
        cached: sum.cached + number(usage, "cached_input_tokens"),
        output: sum.output + number(usage, "output_tokens"),
    }))
}

/// Signs that a call failed because a usage or rate limit was reached. Each
/// CLI words this differently and neither documents a stable format, so this
/// looks for the phrases both use.
const LIMIT_SIGNS: &[&str] = &["rate limit", "rate_limit", "usage limit", "usage_limit", "too many requests", "429"];

fn limit_sign(text: &str) -> Option<String> {
    text.lines()
        .find(|line| {
            let lower = line.to_lowercase();
            LIMIT_SIGNS.iter().any(|sign| lower.contains(sign))
        })
        .map(|line| line.trim().chars().take(240).collect())
}

/// Why a reply with no usable output looks like a usage limit, if it does.
/// A limit is a reason to wait, not a failure of the work.
pub fn rate_limited(reply: &Reply) -> Option<String> {
    if reply.output.is_some() && reply.error.is_none() {
        return None;
    }
    let read = |path: &Path| std::fs::read_to_string(path).unwrap_or_default();
    let stdout = read(&reply.stdout_path);
    // Claude says so itself, in the event it prints with each reply.
    if let Some(info) = claude_limits(&stdout).filter(|info| info.get("status").and_then(Value::as_str) == Some("rejected")) {
        let window = info.get("rateLimitType").and_then(Value::as_str).unwrap_or("usage");
        return Some(format!("claude: the {} limit is reached", window.replace('_', " ")));
    }
    // In stdout only lines that report an error count: a prompt echoed back
    // or a file the agent printed could mention a limit for other reasons.
    // Claude's stream has a line per message, and its own word on limits was
    // read above, so of a stream only the result line counts.
    let results: Vec<&str> = stdout.lines().filter(|line| is_claude_result(line)).collect();
    let lines: Vec<&str> = if results.is_empty() { stdout.lines().collect() } else { results };
    let stdout_errors: String = lines.into_iter().filter(|l| l.to_lowercase().contains("error")).collect::<Vec<_>>().join("\n");
    reply
        .error
        .as_deref()
        .and_then(limit_sign)
        .or_else(|| limit_sign(&read(&reply.stderr_path)))
        .or_else(|| limit_sign(&stdout_errors))
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
    if let Some(model) = &call.runner.model {
        args.extend(["-m".into(), model.clone()]);
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
    let mut tools: Vec<&str> = tools.to_vec();
    if call.web {
        tools.extend(["WebSearch", "WebFetch"]);
    }
    // The stream, not the single result: it carries the same result as its
    // last line, and before it an event that says how much of the plan is
    // used. The stream needs --verbose in print mode.
    let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--json-schema", call.schema, "--settings", CLAUDE_SETTINGS]
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
    if let Some(model) = &call.runner.model {
        args.extend(["--model".into(), model.clone()]);
    }
    args.push("--allowedTools".into());
    args.extend(tools.iter().map(|t| t.to_string()));
    args
}

/// Antigravity takes the prompt as an argument. Its sandbox is the same for
/// a reader and a writer, and it has no way to continue a session here.
fn agy_args(call: &Call) -> Vec<String> {
    let minutes = format!("{}m", (call.timeout.as_secs() / 60).max(1));
    let mut args: Vec<String> = ["-p", call.prompt, "--output-format", "json", "--json-schema", call.schema, "--sandbox", "--print-timeout", &minutes].iter().map(|s| s.to_string()).collect();
    if let Some(model) = &call.runner.model {
        args.extend(["--model".into(), model.clone()]);
    }
    args
}

/// Antigravity prints one JSON result. A tool it was not allowed to use does
/// not fail the run: it exits 0 with an empty response and says so on stderr.
fn read_agy(stdout: &str, stderr: &str) -> (Option<Value>, Option<String>) {
    let Ok(result) = serde_json::from_str::<Value>(stdout.trim()) else {
        let detail = stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("no detail");
        return (None, Some(format!("agy did not print a JSON result: {}", detail.trim())));
    };
    if result.get("status").and_then(Value::as_str) != Some("SUCCESS") {
        let detail = result.get("error").and_then(Value::as_str).unwrap_or("no detail");
        return (None, Some(format!("agy reported an error: {}", detail.lines().next().unwrap_or(detail))));
    }
    match result.get("structured_output") {
        Some(value) if !value.is_null() => (Some(value.clone()), None),
        _ if stderr.contains("auto-denied") => (None, Some("agy was refused a tool and returned nothing".into())),
        _ => (None, Some("agy returned no structured output".into())),
    }
}

/// Codex writes its final message to a file and its events to stdout.
fn read_codex(stdout: &str, output: &Path) -> (Option<Value>, Option<String>, Option<String>) {
    let session = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|event| event.get("thread_id").and_then(Value::as_str).map(str::to_string));
    // What Codex itself said went wrong, when it said: a failed turn, or
    // an error event. That says more than the state of its output file.
    let said = || {
        stdout.lines().rev().filter_map(|line| serde_json::from_str::<Value>(line).ok()).find_map(|event| match event.get("type").and_then(Value::as_str) {
            Some("turn.failed") => event.get("error").and_then(|error| error.get("message")).and_then(Value::as_str).map(str::to_string),
            Some("error") => event.get("message").and_then(Value::as_str).map(str::to_string),
            _ => None,
        })
    };
    match std::fs::read_to_string(output) {
        Err(_) => (None, session, Some(said().map_or("codex wrote no final message".to_string(), |why| format!("codex reported an error: {why}")))),
        Ok(raw) => match serde_json::from_str::<Value>(raw.trim()) {
            Ok(value) => (Some(value), session, None),
            Err(e) => (None, session, Some(said().map_or(format!("codex's final message is not JSON: {e}"), |why| format!("codex reported an error: {why}")))),
        },
    }
}

/// Where Codex keeps its sessions: the runner's own folder, or what the
/// environment names, or `~/.codex`.
fn codex_home(runner: &Runner) -> PathBuf {
    if let Some(dir) = &runner.config_dir {
        return dir.clone();
    }
    // A runner with its own env file starts without the inherited variable.
    if runner.env_file.is_none() {
        if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|dir| !dir.is_empty()) {
            return PathBuf::from(dir);
        }
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".codex")
}

/// The newest `rate_limits` record in the file Codex keeps for a session:
/// `sessions/<year>/<month>/<day>/rollout-<time>-<session>.jsonl`. Codex
/// writes one after each reply. This reads the file and nothing else of
/// Codex's: no login, no token.
fn codex_limits(home: &Path, session: &str) -> Option<Value> {
    // A session id is letters, digits and dashes. Anything else is not looked for.
    if session.is_empty() || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let ending = format!("-{session}.jsonl");
    let mut folders = vec![(home.join("sessions"), 0)];
    let mut file = None;
    while let Some((folder, depth)) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 3 {
                folders.push((path, depth + 1));
            } else if entry.file_name().to_string_lossy().ends_with(&ending) {
                file = Some(path);
            }
        }
    }
    let text = std::fs::read_to_string(file?).ok()?;
    text.lines().rev().filter(|line| line.contains("rate_limits")).filter_map(|line| serde_json::from_str::<Value>(line).ok()).find_map(|record| {
        let payload = record.get("payload")?;
        [payload.get("rate_limits"), payload.get("info").and_then(|info| info.get("rate_limits"))].into_iter().flatten().find(|limits| limits.is_object()).cloned()
    })
}

fn is_claude_result(line: &str) -> bool {
    serde_json::from_str::<Value>(line).is_ok_and(|value| value.get("type").and_then(Value::as_str) == Some("result"))
}

/// The `rate_limit_info` of the last limit event in Claude's stream.
fn claude_limits(stdout: &str) -> Option<Value> {
    stdout
        .lines()
        .rev()
        .filter(|line| line.contains("rate_limit_event"))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("rate_limit_event"))
        .find_map(|event| event.get("rate_limit_info").filter(|info| info.is_object()).cloned())
}

/// Claude prints a line per message. The last one of type `result` holds the
/// schema-checked reply. A single result with nothing before it reads the same.
fn read_claude(stdout: &str) -> (Option<Value>, Option<String>, Option<String>) {
    let found = stdout.lines().rev().filter_map(|line| serde_json::from_str::<Value>(line).ok()).find(|value| value.get("type").and_then(Value::as_str) == Some("result"));
    let Some(result) = found.or_else(|| serde_json::from_str::<Value>(stdout.trim()).ok()) else {
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
    fn claude_s_stream_gives_the_result_and_what_it_said_about_the_plan() {
        let stream = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s9\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":\"working\"}}\n",
            "{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"allowed\",\"rateLimitType\":\"five_hour\",\"unifiedWindows\":{\"five_hour\":{\"utilization\":0.5,\"resetsAt\":7200,\"status\":\"allowed\"}}}}\n",
            "{\"type\":\"result\",\"is_error\":false,\"session_id\":\"s9\",\"structured_output\":{\"verdict\":\"approve\"}}\n",
        );
        let (output, session, error) = read_claude(stream);
        assert_eq!((output.unwrap()["verdict"].as_str(), session.as_deref(), error), (Some("approve"), Some("s9"), None));
        let info = claude_limits(stream).unwrap();
        assert_eq!(ledger::read_claude(&info, 100).unwrap().windows, ["5-hour window: 50% used, resets 1970-01-01 02:00 UTC"]);
        // A stream that was cut off before its result is not a reply.
        let cut = stream.lines().take(3).collect::<Vec<_>>().join("\n");
        assert_eq!(read_claude(&cut).2.as_deref(), Some("claude did not print a JSON result"));
        // No event, no reading. Text that only mentions one is not an event.
        assert_eq!(claude_limits("{\"type\":\"assistant\",\"message\":\"a rate_limit_event is printed\"}"), None);
    }

    #[test]
    fn codex_s_own_word_on_a_failure_is_what_is_reported() {
        let dir = std::env::temp_dir().join(format!("factory-codex-said-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.json");
        // A model the account can't use: Codex says so, and leaves an empty file.
        std::fs::write(&out, "").unwrap();
        let stdout = "{\"type\":\"thread.started\",\"thread_id\":\"t1\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"error\",\"message\":\"a warning\"}}\n{\"type\":\"error\",\"message\":\"The 'gpt-0' model is not supported\"}\n{\"type\":\"turn.failed\",\"error\":{\"message\":\"The 'gpt-0' model is not supported when using Codex with a ChatGPT account.\"}}\n";
        let (output, session, error) = read_codex(stdout, &out);
        assert_eq!((output, session.as_deref()), (None, Some("t1")));
        assert_eq!(error.as_deref(), Some("codex reported an error: The 'gpt-0' model is not supported when using Codex with a ChatGPT account."));
        // With nothing said, the state of the file is all there is to report.
        assert!(read_codex("{\"type\":\"thread.started\",\"thread_id\":\"t1\"}\n", &out).2.unwrap().starts_with("codex's final message is not JSON"));
        assert_eq!(read_codex("", &dir.join("missing.json")).2.as_deref(), Some("codex wrote no final message"));
        // A reply that is there is a reply, whatever was said along the way.
        std::fs::write(&out, "{\"ok\":true}").unwrap();
        assert_eq!(read_codex(stdout, &out).2, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_counts_are_read_from_what_either_program_prints() {
        // Claude: cached input is counted apart, so the input is the sum.
        let claude = "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":999}}}\n{\"type\":\"result\",\"usage\":{\"input_tokens\":4,\"cache_creation_input_tokens\":7104,\"cache_read_input_tokens\":82402,\"output_tokens\":653}}\n";
        assert_eq!(tokens(claude), Some(Tokens { input: 89_510, cached: 82_402, output: 653 }));
        // Codex: one record per turn, cached input a part of the input.
        let codex = "{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":224123,\"cached_input_tokens\":185856,\"output_tokens\":809}}\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1000,\"cached_input_tokens\":0,\"output_tokens\":10}}\n";
        assert_eq!(tokens(codex), Some(Tokens { input: 225_123, cached: 185_856, output: 819 }));
        // A call that failed before it counted anything has no count.
        assert_eq!(tokens("{\"type\":\"error\",\"message\":\"usage limit\"}"), None);
        assert_eq!(tokens(""), None);
    }

    #[test]
    fn codex_s_session_file_says_how_much_of_the_plan_is_used() {
        let home = std::env::temp_dir().join(format!("factory-codex-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let day = home.join("sessions/2026/11/24");
        std::fs::create_dir_all(&day).unwrap();
        let record = |percent: u32| format!("{{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":null,\"rate_limits\":{{\"primary\":{{\"used_percent\":{percent}.0,\"window_minutes\":10080,\"resets_at\":9000}},\"secondary\":null}}}}}}");
        // The newest record of the session's own file is the one that counts.
        std::fs::write(day.join("rollout-2026-11-24T22-10-26-abc-123.jsonl"), format!("{{\"type\":\"session_meta\"}}\n{}\n{{\"type\":\"response_item\"}}\n{}\n", record(2), record(7))).unwrap();
        std::fs::write(day.join("rollout-2026-11-24T22-11-00-other-9.jsonl"), record(99)).unwrap();
        let limits = codex_limits(&home, "abc-123").unwrap();
        assert_eq!(limits["primary"]["used_percent"], 7.0);
        assert_eq!(ledger::read_codex(&limits, 100).unwrap().windows, ["weekly window: 7% used, resets 1970-01-01 02:30 UTC"]);
        // Another session, a session with no file, and an id that is no id.
        assert_eq!(codex_limits(&home, "other-9").unwrap()["primary"]["used_percent"], 99.0);
        assert_eq!(codex_limits(&home, "nobody"), None);
        assert_eq!(codex_limits(&home, "../abc-123"), None);
        assert_eq!(codex_limits(&home.join("missing"), "abc-123"), None);
        // The runner's own folder is where its sessions are.
        let own = Runner { config_dir: Some(home.clone()), ..Runner::of(Vendor::Codex, None) };
        assert_eq!(codex_home(&own), home);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_usage_limit_is_told_apart_from_other_failures() {
        let dir = std::env::temp_dir().join(format!("factory-limit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let reply = |error: Option<&str>, stdout: &str, stderr: &str, output: bool| {
            std::fs::write(dir.join("out"), stdout).unwrap();
            std::fs::write(dir.join("err"), stderr).unwrap();
            Reply {
                outcome: Outcome { exit_code: Some(1), timed_out: false, duration: Duration::from_secs(1) },
                output: output.then(|| serde_json::json!({})),
                error: error.map(str::to_string),
                input_path: dir.join("in"),
                output_path: dir.join("o"),
                stdout_path: dir.join("out"),
                stderr_path: dir.join("err"),
                session_id: None,
                quota: None,
            }
        };
        // Claude reports it in its result. Codex prints an error event and exits 1.
        assert!(rate_limited(&reply(Some("claude reported an error: API Error: Rate limit reached"), "", "", false)).is_some());
        let event = "{\"type\":\"error\",\"message\":\"You've hit your usage limit. Try again at 3:10 PM.\"}";
        assert!(rate_limited(&reply(Some("codex wrote no final message"), event, "", false)).unwrap().contains("usage limit"));
        assert!(rate_limited(&reply(Some("codex exited with Some(1)"), "", "stream error: 429 Too Many Requests", false)).is_some());
        // Other failures, and work that merely mentions limits, are not limits.
        assert!(rate_limited(&reply(Some("claude reported an error: 401 OAuth access token is invalid"), "", "", false)).is_none());
        let chatter = "{\"type\":\"item\",\"text\":\"I added a rate limit to the API client\"}";
        assert!(rate_limited(&reply(Some("codex wrote no final message"), chatter, "", false)).is_none());
        assert!(rate_limited(&reply(None, event, "rate limit", true)).is_none());
        // In Claude's stream only its own word and the result line count. An
        // event that says the plan is fine is not a limit, whatever else
        // failed, and neither is a message that talks about one.
        let fine = "{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"allowed\",\"rateLimitType\":\"five_hour\",\"errorCode\":null}}";
        let talk = "{\"type\":\"assistant\",\"message\":\"an error: the API answers 429 when the rate limit is reached\"}";
        let broken = "{\"type\":\"result\",\"is_error\":true,\"result\":\"error: the reply is not valid JSON\"}";
        let stream = format!("{fine}\n{talk}\n{broken}\n");
        assert!(rate_limited(&reply(Some("claude reported an error: the reply is not valid JSON"), &stream, "", false)).is_none());
        // When Claude says the window is used up, that is the reason.
        let used_up = fine.replace("allowed", "rejected");
        let stream = format!("{used_up}\n{broken}\n");
        assert_eq!(rate_limited(&reply(Some("claude reported an error"), &stream, "", false)).as_deref(), Some("claude: the five hour limit is reached"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sandbox_flags_follow_access() {
        let dir = std::env::temp_dir();
        let (codex_runner, claude_runner) = (Runner::of(Vendor::Codex, None), Runner::of(Vendor::Claude, None));
        let call = Call {
            runner: &codex_runner,
            access: Access::WorkspaceWrite,
            cwd: &dir,
            prompt: "p",
            schema: "{}",
            dir: &dir,
            stem: "s",
            timeout: Duration::from_secs(1),
            resume: None,
            web: false,
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

        let review = Call { runner: &claude_runner, access: Access::ReadOnly, resume: None, ..call };
        let claude = claude_args(&review).join(" ");
        assert!(claude.contains("--permission-mode dontAsk"), "{claude}");
        assert!(!claude.contains("Edit") && !claude.contains("Write ") && !claude.contains("Bash"), "{claude}");

        // Every Claude call is confined to its directory, and never gets a
        // shell that could run a script it edited.
        let writer = Call { runner: &claude_runner, resume: Some("s-9"), ..call };
        let claude = claude_args(&writer).join(" ");
        for args in [&claude, &claude_args(&review).join(" ")] {
            assert!(args.contains("blockWritesOutsideWorkingDirectories\":true"), "{args}");
            assert!(args.contains("blockReadsOutsideWorkingDirectories\":true"), "{args}");
            assert!(!args.contains("verify.sh") && !args.contains("git "), "{args}");
        }
        assert!(claude.contains("--resume s-9") && claude.contains("--permission-mode acceptEdits"), "{claude}");
        // Web tools are given only when a module asks for them.
        assert!(!claude.contains("WebSearch"), "{claude}");
        let researcher = Call { runner: &claude_runner, web: true, resume: None, ..call };
        let claude = claude_args(&researcher).join(" ");
        assert!(claude.contains("WebSearch WebFetch") && claude.contains("blockWritesOutsideWorkingDirectories"), "{claude}");
    }

    fn call_by<'a>(runner: &'a Runner, dir: &'a Path) -> Call<'a> {
        Call { runner, access: Access::WorkspaceWrite, cwd: dir, prompt: "do it", schema: "{}", dir, stem: "t", timeout: Duration::from_secs(300), resume: None, web: false }
    }

    #[test]
    fn each_program_is_started_confined_with_its_model() {
        let dir = PathBuf::from("/work");
        let (schema, out) = (Path::new("s.json"), Path::new("o.json"));
        let runner = |cli, model: &str| Runner { cli, model: Some(model.into()), env_file: None, config_dir: None };

        let codex = launch(&call_by(&runner(Cli::Codex, "m-1"), &dir), schema, out).unwrap();
        assert_eq!((codex.program.as_str(), codex.stdin.as_deref()), ("codex", Some("do it")));
        let line = codex.args.join(" ");
        assert!(line.contains("--sandbox workspace-write") && line.contains("-m m-1") && !line.contains("danger"), "{line}");

        let other_login = Runner { config_dir: Some("/logins/b".into()), ..runner(Cli::Claude, "m-1") };
        let claude = launch(&call_by(&other_login, &dir), schema, out).unwrap();
        let line = claude.args.join(" ");
        assert!(line.contains("blockWritesOutsideWorkingDirectories") && line.contains("--model m-1"), "{line}");
        assert_eq!(claude.env, vec![("CLAUDE_CONFIG_DIR".to_string(), "/logins/b".to_string())]);

        let agy = launch(&call_by(&runner(Cli::Agy, "m-1"), &dir), schema, out).unwrap();
        let line = agy.args.join(" ");
        assert!(line.starts_with("-p do it") && line.contains("--sandbox") && line.contains("--model m-1") && line.contains("--print-timeout 5m"), "{line}");
        assert!(!line.contains("skip-permissions") && agy.stdin.is_none());
        assert!(launch(&call_by(&Runner { config_dir: Some("/x".into()), ..runner(Cli::Agy, "m") }, &dir), schema, out).is_err());
    }

    #[test]
    fn an_env_file_is_read_by_the_shell_not_the_engine() {
        let dir = PathBuf::from("/work");
        let glm = Runner { cli: Cli::Claude, model: Some("opus".into()), env_file: Some("/keys/env.sh".into()), config_dir: None };
        let launched = launch(&call_by(&glm, &dir), Path::new("s.json"), Path::new("o.json")).unwrap();
        assert_eq!(launched.program, "sh");
        assert_eq!(&launched.args[..4], ["-c", WITH_ENV_FILE, "/keys/env.sh", "claude"]);
        assert!(launched.args.contains(&"--json-schema".to_string()) && launched.env.is_empty());
    }

    #[test]
    fn the_shell_wrapper_exports_what_the_file_sets() {
        let dir = std::env::temp_dir().join(format!("factory-envfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("env.sh"), "A_KEY=from-file\n").unwrap();
        let run = |file: &str| std::process::Command::new("sh").args(["-c", WITH_ENV_FILE, file, "sh", "-c", "printf %s \"$A_KEY\""]).output().unwrap();
        let found = run(&dir.join("env.sh").display().to_string());
        assert_eq!(String::from_utf8_lossy(&found.stdout), "from-file");
        assert_eq!(run(&dir.join("missing.sh").display().to_string()).status.code(), Some(ENV_FILE_UNREADABLE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_runner_with_its_own_login_inherits_no_other() {
        let names = || ["PATH", "HOME", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_ENTRYPOINT", "CLAUDECODE", "OPENAI_API_KEY", "CODEX_HOME", "GEMINI_API_KEY", "MY_CLAUDE"].iter().map(|s| s.to_string());
        assert_eq!(login_variables(Cli::Claude, names()), ["ANTHROPIC_BASE_URL", "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"]);
        assert_eq!(login_variables(Cli::Codex, names()), ["CODEX_HOME", "OPENAI_API_KEY"]);
        assert_eq!(login_variables(Cli::Agy, names()), ["GEMINI_API_KEY"]);
        // The default login keeps the environment as it is.
        let dir = PathBuf::from("/work");
        assert!(launch(&call_by(&Runner::of(Vendor::Claude, None), &dir), Path::new("s"), Path::new("o")).unwrap().unset.is_empty());
    }

    #[test]
    fn an_agy_run_that_was_refused_a_tool_is_not_a_success() {
        let ok = r#"{"status":"SUCCESS","response":"{}","structured_output":{"wrote":true}}"#;
        assert_eq!(read_agy(ok, ""), (Some(serde_json::json!({"wrote": true})), None));
        let empty = r#"{"status":"SUCCESS","response":""}"#;
        assert!(read_agy(empty, "jetski: a tool required the \"write_file\" permission, so it was auto-denied").1.unwrap().contains("refused"));
        assert!(read_agy(empty, "").1.unwrap().contains("no structured output"));
        assert!(read_agy(r#"{"status":"ERROR","error":"model x is not recognized\nAvailable"}"#, "").1.unwrap().ends_with("model x is not recognized"));
        assert!(read_agy("", "authentication required").1.unwrap().contains("authentication required"));
    }
}
