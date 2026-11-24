//! What the engine does with a judgment: an LLM picks from a fixed menu, and
//! code decides whether that pick is allowed.
//!
//! No menu item skips a gate. `retry` and `switch-vendor` start the issue
//! over, through every check again. `split` only proposes a plan for a
//! person to approve. Anything else, and anything malformed, is `escalate`.

use serde::{Deserialize, Serialize};

/// How often a judgment may send one issue around again, by default and at
/// most. The limit can be lowered, never removed.
pub const DEFAULT_JUDGMENTS: u32 = 2;
pub const MAX_JUDGMENTS: u32 = 3;

/// One smaller issue of a proposed split.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitIssue {
    pub key: String,
    pub title: String,
    pub summary: String,
    pub scope: Vec<String>,
    pub accept: Vec<String>,
    pub flows: Vec<String>,
    pub accessibility_ids: Vec<String>,
    pub depends_on: Vec<String>,
}

/// The judgment's final message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub action: String,
    pub reason: String,
    #[serde(default)]
    pub split: Vec<SplitIssue>,
}

/// What the engine will do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Start the issue over with the same agents.
    Retry,
    /// Start the issue over with the vendors swapped.
    SwitchVendor,
    /// Answer an open question, then start the issue over.
    Research,
    /// Propose these issues to a person as a plan.
    Split(Vec<SplitIssue>),
    /// A person decides.
    Escalate,
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Retry => "retry",
            Action::SwitchVendor => "switch-vendor",
            Action::Research => "research",
            Action::Split(_) => "split",
            Action::Escalate => "escalate",
        }
    }
}

/// What may be chosen right now.
#[derive(Debug, Clone, Copy)]
pub struct Menu {
    /// New attempts still allowed for this issue.
    pub attempts_left: u32,
    /// Whether the workflow has a research module.
    pub research: bool,
}

impl Menu {
    /// The sentence the prompt gives the LLM.
    pub fn describe(&self) -> String {
        let mut available = Vec::new();
        let mut unavailable = Vec::new();
        if self.attempts_left > 0 {
            available.extend(["`retry`", "`switch-vendor`"]);
            if self.research {
                available.push("`research`");
            }
        } else {
            unavailable.push("`retry`, `switch-vendor` and `research` (this issue has used every new attempt it is allowed)");
        }
        if self.attempts_left > 0 && !self.research {
            unavailable.push("`research` (this repository's workflow has no research step)");
        }
        available.extend(["`split`", "`escalate`"]);
        let mut out = format!("Available now: {}.", available.join(", "));
        if !unavailable.is_empty() {
            out.push_str(&format!(" Not available: {}.", unavailable.join(", and ")));
        }
        out
    }
}

/// Turns the LLM's reply into an action. The second value is the reason a
/// person is shown: the LLM's own, or why the engine overruled it.
pub fn settle(reply: Result<Decision, String>, menu: Menu) -> (Action, String) {
    let decision = match reply {
        Ok(decision) => decision,
        Err(problem) => return (Action::Escalate, format!("The judgment gave no usable answer ({problem}), so a person decides.")),
    };
    let reason = decision.reason.trim().to_string();
    let overruled = |why: &str| (Action::Escalate, format!("The judgment chose `{}`, but {why}, so a person decides. Its reason: {reason}", decision.action));
    match decision.action.trim() {
        "escalate" => (Action::Escalate, reason),
        "retry" | "switch-vendor" | "research" if menu.attempts_left == 0 => overruled("this issue has no new attempts left"),
        "retry" => (Action::Retry, reason),
        "switch-vendor" => (Action::SwitchVendor, reason),
        "research" if !menu.research => overruled("the workflow has no research step"),
        "research" => (Action::Research, reason),
        "split" if decision.split.len() < 2 => overruled("a split needs at least two issues"),
        "split" => (Action::Split(decision.split), reason),
        _ => overruled("that is not on the menu"),
    }
}

/// The plan file for a split, in the format `plan validate` checks.
pub fn split_plan(plan_id: &str, issue_number: u64, issue_title: &str, reason: &str, issues: &[SplitIssue]) -> Result<String, String> {
    #[derive(Serialize)]
    struct Flow<'a> {
        file: &'a str,
        new: bool,
    }
    #[derive(Serialize)]
    struct Issue<'a> {
        key: &'a str,
        title: &'a str,
        summary: &'a str,
        scope: &'a [String],
        accept: &'a [String],
        #[serde(skip_serializing_if = "Vec::is_empty")]
        flows: Vec<Flow<'a>>,
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        accessibility_ids: &'a [String],
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        depends_on: &'a [String],
    }
    #[derive(Serialize)]
    struct Plan<'a> {
        plan: &'a str,
        title: String,
        summary: String,
        issues: Vec<Issue<'a>>,
    }
    let plan = Plan {
        plan: plan_id,
        title: format!("Split of #{issue_number}: {}", issue_title.trim()),
        summary: format!("Proposed by the judgment step after issue #{issue_number} could not be finished. {}", reason.trim()),
        issues: issues
            .iter()
            .map(|i| Issue {
                key: i.key.trim(),
                title: i.title.trim(),
                summary: i.summary.trim(),
                scope: &i.scope,
                accept: &i.accept,
                flows: i.flows.iter().map(|file| Flow { file: file.trim(), new: true }).collect(),
                accessibility_ids: &i.accessibility_ids,
                depends_on: &i.depends_on,
            })
            .collect(),
    };
    crate::yaml::to_string(&plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(action: &str, split: usize) -> Decision {
        let issue = |n: usize| SplitIssue {
            key: format!("part-{n}"),
            title: format!("Part {n}"),
            summary: "One half.".into(),
            scope: vec![format!("Sources/P{n}/")],
            accept: vec!["It works".into()],
            flows: if n == 1 { vec!["part_one.yaml".into()] } else { Vec::new() },
            accessibility_ids: Vec::new(),
            depends_on: if n == 2 { vec!["part-1".into()] } else { Vec::new() },
        };
        Decision { action: action.into(), reason: "The developer went in circles.".into(), split: (1..=split).map(issue).collect() }
    }

    const OPEN: Menu = Menu { attempts_left: 1, research: false };
    const SPENT: Menu = Menu { attempts_left: 0, research: true };

    #[test]
    fn the_menu_is_fixed() {
        assert_eq!(settle(Ok(decision("retry", 0)), OPEN).0, Action::Retry);
        assert_eq!(settle(Ok(decision("switch-vendor", 0)), OPEN).0, Action::SwitchVendor);
        assert_eq!(settle(Ok(decision("escalate", 0)), OPEN), (Action::Escalate, "The developer went in circles.".to_string()));
        assert!(matches!(settle(Ok(decision("split", 2)), OPEN).0, Action::Split(list) if list.len() == 2));
        // Anything else is a person's call, and so is an answer that can't be read.
        for off_menu in ["merge-anyway", "skip-verify", "approve", ""] {
            let (action, why) = settle(Ok(decision(off_menu, 0)), OPEN);
            assert_eq!(action, Action::Escalate, "{off_menu}");
            assert!(why.contains("not on the menu"), "{why}");
        }
        let (action, why) = settle(Err("two unusable replies".into()), OPEN);
        assert_eq!(action, Action::Escalate);
        assert!(why.contains("no usable answer"), "{why}");
    }

    #[test]
    fn a_judgment_cannot_grant_itself_more_attempts_or_missing_steps() {
        for action in ["retry", "switch-vendor", "research"] {
            let (found, why) = settle(Ok(decision(action, 0)), SPENT);
            assert_eq!(found, Action::Escalate, "{action}");
            assert!(why.contains("no new attempts left") && why.contains("went in circles"), "{why}");
        }
        assert!(settle(Ok(decision("research", 0)), OPEN).1.contains("no research step"));
        assert_eq!(settle(Ok(decision("research", 0)), Menu { attempts_left: 1, research: true }).0, Action::Research);
        // A split of one issue is not a split. It is still allowed with no attempts left.
        assert!(settle(Ok(decision("split", 1)), OPEN).1.contains("at least two issues"));
        assert!(matches!(settle(Ok(decision("split", 2)), SPENT).0, Action::Split(_)));
    }

    #[test]
    fn the_prompt_says_what_is_available() {
        assert_eq!(OPEN.describe(), "Available now: `retry`, `switch-vendor`, `split`, `escalate`. Not available: `research` (this repository's workflow has no research step).");
        assert!(SPENT.describe().starts_with("Available now: `split`, `escalate`. Not available: `retry`"));
        assert!(Menu { attempts_left: 2, research: true }.describe().ends_with("`research`, `split`, `escalate`."));
    }

    #[test]
    fn a_split_becomes_a_plan_file_the_validator_reads() {
        let text = split_plan("todo-v1-split-7", 7, "Filter todos", "It mixed two changes.", &decision("split", 2).split).unwrap();
        let plan = crate::plan::model::parse_str(&text).unwrap();
        assert_eq!(plan.plan.as_deref(), Some("todo-v1-split-7"));
        assert_eq!(plan.issues.len(), 2);
        assert_eq!(plan.issues[1].depends_on.len(), 1);
        assert!(plan.issues[0].flows[0].is_new());
        assert!(text.contains("Split of #7: Filter todos"), "{text}");
    }
}
