//! The plan.yaml format.
//!
//! Required fields are `Option` here on purpose: the validator reports every
//! missing field at once, which a serde error on the first one would not.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFile {
    /// Plan ID. Prefixes every issue key, for example `todo-v1/add`.
    pub plan: Option<String>,
    /// Title of the parent issue.
    pub title: Option<String>,
    pub summary: Option<String>,
    #[serde(default)]
    pub issues: Vec<IssueSpec>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueSpec {
    pub key: Option<String>,
    pub title: Option<String>,
    pub summary: Option<String>,
    /// Paths the issue may change. A directory covers everything below it.
    #[serde(default)]
    pub scope: Vec<String>,
    /// Acceptance criteria.
    #[serde(default)]
    pub accept: Vec<String>,
    /// Maestro flows that must pass.
    #[serde(default)]
    pub flows: Vec<FlowRef>,
    #[serde(default)]
    pub accessibility_ids: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<DepRef>,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FlowRef {
    Name(String),
    Detailed {
        file: String,
        #[serde(default)]
        new: bool,
    },
}

impl FlowRef {
    pub fn file(&self) -> &str {
        match self {
            FlowRef::Name(f) => f,
            FlowRef::Detailed { file, .. } => file,
        }
    }

    pub fn is_new(&self) -> bool {
        matches!(self, FlowRef::Detailed { new: true, .. })
    }
}

/// A raw `depends_on` entry. A bare number is accepted because an unquoted
/// `#42` is a YAML comment.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DepRef {
    Number(u64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dep {
    /// A key in the same plan.
    Local(String),
    /// `plan-id/key` in another plan.
    Cross { plan: String, key: String },
    /// An existing GitHub issue, written `"#42"` or `42`.
    Issue(u64),
}

impl DepRef {
    pub fn display(&self) -> String {
        match self {
            DepRef::Number(n) => format!("#{n}"),
            DepRef::Text(t) => t.clone(),
        }
    }

    pub fn parse(&self) -> Result<Dep, String> {
        let text = match self {
            DepRef::Number(n) => return Ok(Dep::Issue(*n)),
            DepRef::Text(t) => t.trim(),
        };
        if let Some(num) = text.strip_prefix('#') {
            return num
                .parse::<u64>()
                .map(Dep::Issue)
                .map_err(|_| format!("depends_on '{text}' is not an issue number"));
        }
        match text.split('/').collect::<Vec<_>>().as_slice() {
            [key] if is_key(key) => Ok(Dep::Local(key.to_string())),
            [plan, key] if is_plan_id(plan) && is_key(key) => Ok(Dep::Cross {
                plan: plan.to_string(),
                key: key.to_string(),
            }),
            _ => Err(format!(
                "depends_on '{text}' must be a key, 'plan-id/key', or an issue number like \"#42\""
            )),
        }
    }
}

/// Lowercase letters, digits, `-` and `_`, starting with a letter or digit.
pub fn is_key(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Same as a key, and `.` is allowed so versions like `todo-v1.1` work.
pub fn is_plan_id(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.'
        })
}

#[derive(Debug, Clone)]
pub struct LoadedPlan {
    pub path: PathBuf,
    pub file: PlanFile,
}

#[derive(Debug, Default)]
pub struct PlanSet {
    pub plans: Vec<LoadedPlan>,
    /// Files that could not be read or parsed.
    pub file_errors: Vec<(PathBuf, String)>,
}

/// The single entry point for YAML parsing.
pub fn parse_str(text: &str) -> Result<PlanFile, String> {
    serde_yaml_ng::from_str::<PlanFile>(text).map_err(|e| e.to_string())
}

/// Expands directories to the `.yaml` and `.yml` files directly inside them.
pub fn plan_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found = Vec::new();
            for entry in fs::read_dir(path).with_context(|| format!("reading {}", path.display()))? {
                let p = entry?.path();
                if p.is_file() && is_yaml(&p) {
                    found.push(p);
                }
            }
            found.sort();
            out.extend(found);
        } else if path.exists() {
            out.push(path.clone());
        } else {
            anyhow::bail!("{} does not exist", path.display());
        }
    }
    Ok(out)
}

fn is_yaml(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yaml") | Some("yml")
    )
}

pub fn load(paths: &[PathBuf]) -> Result<PlanSet> {
    let mut set = PlanSet::default();
    for path in plan_files(paths)? {
        match fs::read_to_string(&path) {
            Err(e) => set.file_errors.push((path, e.to_string())),
            Ok(text) => match parse_str(&text) {
                Ok(file) => set.plans.push(LoadedPlan { path, file }),
                Err(e) => set.file_errors.push((path, e)),
            },
        }
    }
    Ok(set)
}
