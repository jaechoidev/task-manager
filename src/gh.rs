//! GitHub access through the `gh` CLI, the same tool the agents use.
//! Everything goes through `gh api`, so no feature depends on a `gh` version.

use serde_json::Value;
use std::fmt;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct ApiError {
    /// HTTP status, when `gh` reported one.
    pub status: Option<u16>,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    pub fn is(&self, status: u16) -> bool {
        self.status == Some(status)
    }
}

/// Calls one REST endpoint. A response with no body gives `Value::Null`.
pub fn api(method: &str, path: &str, body: Option<&Value>) -> Result<Value, ApiError> {
    let mut cmd = Command::new("gh");
    cmd.args(["api", "--method", method, "-H", "Accept: application/vnd.github+json", path]);
    if body.is_some() {
        cmd.args(["--input", "-"]);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let fail = |message: String| ApiError { status: None, message };
    let mut child = cmd.spawn().map_err(|e| fail(format!("could not run gh: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(body) = body {
            stdin
                .write_all(body.to_string().as_bytes())
                .map_err(|e| fail(format!("could not send the request body to gh: {e}")))?;
        }
    }
    let output = child.wait_with_output().map_err(|e| fail(format!("gh did not finish: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let status = stderr
            .rsplit_once("(HTTP ")
            .and_then(|(_, rest)| rest.split(')').next())
            .and_then(|code| code.trim().parse::<u16>().ok());
        let detail = serde_json::from_str::<Value>(&stdout)
            .ok()
            .and_then(|v| v.get("errors").map(|e| format!(" {e}")))
            .unwrap_or_default();
        return Err(ApiError { status, message: format!("{method} {path}: {stderr}{detail}") });
    }
    if stdout.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&stdout).map_err(|e| fail(format!("{method} {path}: unreadable response: {e}")))
}

/// Reads every page of a list endpoint. `path` must not carry a `page` parameter.
pub fn api_list(path: &str) -> Result<Vec<Value>, ApiError> {
    let sep = if path.contains('?') { '&' } else { '?' };
    let mut all = Vec::new();
    for page in 1.. {
        let value = api("GET", &format!("{path}{sep}per_page=100&page={page}"), None)?;
        let items = value.as_array().cloned().unwrap_or_default();
        let count = items.len();
        all.extend(items);
        if count < 100 {
            break;
        }
    }
    Ok(all)
}

/// Percent-encodes one path segment, for label names like `size:S`.
pub fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The repo to act on: the flag, then `GITHUB_REPOSITORY`, then the checkout.
pub fn resolve_repo(flag: Option<&str>) -> Result<String, ApiError> {
    if let Some(repo) = flag {
        return Ok(repo.to_string());
    }
    if let Ok(repo) = std::env::var("GITHUB_REPOSITORY") {
        if !repo.is_empty() {
            return Ok(repo);
        }
    }
    let fail = |message: String| ApiError { status: None, message };
    let output = Command::new("gh")
        .args(["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
        .output()
        .map_err(|e| fail(format!("could not run gh: {e}")))?;
    let repo = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || repo.is_empty() {
        return Err(fail("could not tell which repository to use; pass --repo owner/name".into()));
    }
    Ok(repo)
}
