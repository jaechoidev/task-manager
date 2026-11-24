//! Reads one issue from GitHub and the contract line `plan create` wrote.

use crate::gh;
use crate::plan::create::{parse_marker, Marker};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub plan: String,
    pub key: String,
    /// The prose between the contract line and the first heading.
    pub summary: String,
    pub marker: Marker,
}

/// The free text an issue starts with, after the contract line.
pub fn summary_of(body: &str) -> String {
    body.lines()
        .skip_while(|line| !line.contains("-->"))
        .skip(1)
        .take_while(|line| !line.starts_with("## ") && line.trim() != "---")
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// The open issues this one is blocked by, from GitHub's native links.
pub fn open_blockers(repo: &str, number: u64) -> Result<Vec<u64>> {
    let blockers = gh::api_list(&format!("repos/{repo}/issues/{number}/dependencies/blocked_by"))?;
    Ok(blockers
        .iter()
        .filter(|b| b.get("state").and_then(Value::as_str) == Some("open"))
        .filter_map(|b| b.get("number").and_then(Value::as_u64))
        .collect())
}

/// Fails unless the issue was made by `plan create`, is open, and nothing it
/// is blocked by is still open.
pub fn load(repo: &str, number: u64) -> Result<Issue> {
    let value = match gh::api("GET", &format!("repos/{repo}/issues/{number}"), None) {
        Ok(value) => value,
        Err(e) if e.is(404) => bail!("issue #{number} does not exist in {repo}"),
        Err(e) => return Err(e.into()),
    };
    if value.get("pull_request").is_some() {
        bail!("#{number} is a pull request, not an issue");
    }
    if value.get("state").and_then(Value::as_str) != Some("open") {
        bail!("issue #{number} is closed");
    }
    let body = value.get("body").and_then(Value::as_str).unwrap_or("");
    let marker = parse_marker(body)
        .ok_or_else(|| anyhow!("issue #{number} was not created by `factory plan create`: its first line is missing"))?;
    let key = marker.key.clone().ok_or_else(|| anyhow!("issue #{number} is a plan's parent issue; run one of its sub-issues"))?;

    let open: Vec<String> = open_blockers(repo, number)?.iter().map(|n| format!("#{n}")).collect();
    if !open.is_empty() {
        bail!("issue #{number} is not ready: it is blocked by {}, still open", open.join(", "));
    }

    Ok(Issue {
        number,
        title: value.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        plan: marker.plan.clone(),
        key,
        summary: summary_of(body),
        marker,
    })
}

#[cfg(test)]
mod tests {
    use super::summary_of;

    #[test]
    fn summary_is_the_text_before_the_first_heading() {
        let body = "<!-- factory:issue {\"v\":1} -->\n\nThe app launches to a list.\nSecond line.\n\n## Scope\n\n- `a/`\n";
        assert_eq!(summary_of(body), "The app launches to a list.\nSecond line.");
        assert_eq!(summary_of("<!-- factory:issue {} -->\n\n## Scope\n"), "");
    }
}
