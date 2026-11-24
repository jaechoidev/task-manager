//! Turns a validation report into text for a terminal or markdown for a PR
//! comment.

use super::validate::{PlanReport, Report};
use std::collections::BTreeSet;
use std::fmt::Write;

/// First line of the PR comment, so the workflow can find and update it.
pub const COMMENT_MARKER: &str = "<!-- factory:plan-validate -->";

fn status(plan: &PlanReport) -> String {
    match plan.errors.len() {
        0 => "OK".to_string(),
        1 => "FAILED (1 error)".to_string(),
        n => format!("FAILED ({n} errors)"),
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// `only` limits the detail to those plan paths. Errors are always shown.
pub fn text(report: &Report, only: Option<&BTreeSet<String>>) -> String {
    let mut out = String::new();
    for diag in &report.file_errors {
        let _ = writeln!(out, "ERROR {}: {}", diag.subject, diag.message);
    }
    if report.plans.is_empty() && report.file_errors.is_empty() {
        out.push_str("no plan files found\n");
    }
    for plan in &report.plans {
        let shown = only.is_none_or(|set| set.contains(&plan.path));
        if !shown && plan.errors.is_empty() {
            continue;
        }
        let _ = writeln!(out, "plan {}: {}, {}", plan.id, plural(plan.issue_count, "issue"), status(plan));
        for (i, batch) in plan.batches.iter().enumerate() {
            let note = if batch.len() > 1 { "   (parallel, scopes don't overlap)" } else { "" };
            let _ = writeln!(out, "batch {}: {}{}", i + 1, batch.join(", "), note);
        }
        let waits: Vec<String> = plan
            .nodes
            .iter()
            .filter(|n| !n.external.is_empty())
            .map(|n| format!("{} waits on {}", n.key, n.external.join(", ")))
            .collect();
        for line in waits {
            let _ = writeln!(out, "outside this plan: {line}");
        }
        if !plan.errors.is_empty() || !plan.warnings.is_empty() {
            out.push('\n');
        }
        for diag in &plan.errors {
            let _ = writeln!(out, "ERROR {}: {}", diag.subject, diag.message);
        }
        for diag in &plan.warnings {
            let _ = writeln!(out, "WARN {}: {}", diag.subject, diag.message);
        }
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

pub fn markdown(report: &Report, only: Option<&BTreeSet<String>>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{COMMENT_MARKER}");
    let verdict = if report.ok { "passed" } else { "failed" };
    let _ = writeln!(out, "## Plan validation {verdict}\n");

    if !report.file_errors.is_empty() {
        out.push_str("**Files that could not be read**\n\n");
        for diag in &report.file_errors {
            let _ = writeln!(out, "- `{}`: {}", diag.subject, diag.message);
        }
        out.push('\n');
    }

    let mut quiet = 0;
    for plan in &report.plans {
        let shown = only.is_none_or(|set| set.contains(&plan.path));
        if !shown && plan.errors.is_empty() {
            quiet += 1;
            continue;
        }
        let title = if plan.title.is_empty() { String::new() } else { format!(": {}", plan.title) };
        let _ = writeln!(out, "### `{}`{}\n", plan.id, title);
        let _ = writeln!(out, "{}, {}. File: `{}`\n", plural(plan.issue_count, "issue"), status(plan), plan.path);

        if !plan.errors.is_empty() {
            out.push_str("**Errors**\n\n");
            for diag in &plan.errors {
                let _ = writeln!(out, "- `{}`: {}", diag.subject, diag.message);
            }
            out.push('\n');
        }
        if !plan.warnings.is_empty() {
            out.push_str("**Warnings**\n\n");
            for diag in &plan.warnings {
                let _ = writeln!(out, "- `{}`: {}", diag.subject, diag.message);
            }
            out.push('\n');
        }

        if !plan.batches.is_empty() {
            out.push_str("| Batch | Issues | Runs |\n| --- | --- | --- |\n");
            for (i, batch) in plan.batches.iter().enumerate() {
                let keys: Vec<String> = batch.iter().map(|k| format!("`{k}`")).collect();
                let runs = if batch.len() > 1 { "in parallel, scopes don't overlap" } else { "alone" };
                let _ = writeln!(out, "| {} | {} | {} |", i + 1, keys.join(", "), runs);
            }
            out.push('\n');
        }

        if !plan.nodes.is_empty() {
            out.push_str("```mermaid\ngraph LR\n");
            let index = |key: &str| plan.nodes.iter().position(|n| n.key == key).unwrap_or(0);
            for (i, node) in plan.nodes.iter().enumerate() {
                let _ = writeln!(out, "  n{i}[\"{}\"]", node.key);
            }
            for (i, node) in plan.nodes.iter().enumerate() {
                for dep in &node.depends_on {
                    let _ = writeln!(out, "  n{} --> n{i}", index(dep));
                }
                for (j, ext) in node.external.iter().enumerate() {
                    let _ = writeln!(out, "  x{i}_{j}([\"{ext}\"]) -.-> n{i}");
                }
            }
            out.push_str("```\n\n");
        }
    }

    if report.plans.is_empty() && report.file_errors.is_empty() {
        out.push_str("No plan files found.\n");
    } else if quiet > 0 {
        let _ = writeln!(out, "{} not changed by this pull request and still valid.", plural(quiet, "other plan"));
    }
    out.trim_end().to_string() + "\n"
}

pub fn json(report: &Report) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string()) + "\n"
}
