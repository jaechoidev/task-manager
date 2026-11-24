//! The loop, as plain logic: which step runs next, when a failure goes back
//! to the developer, when the branch must be checked again, and when the
//! rounds run out.
//!
//! Nothing here calls an agent or touches git, so every path can be tested.

use anyhow::Result;
use std::collections::BTreeMap;

/// Most fix rounds a workflow file may ask for, in either budget. A limit can
/// be lowered, and raised up to this, but never removed.
pub const MAX_FIX_ROUNDS: u32 = 5;
pub const DEFAULT_FIX_ROUNDS: u32 = 3;
pub const DEFAULT_GITHUB_FIX_ROUNDS: u32 = 2;
/// How often one run may take in a moved base branch and check itself again.
pub const MAX_SYNCS: u32 = 3;

/// How one step ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pass,
    /// It failed in a way the developer can fix. The text is what the
    /// developer is told.
    Rework(String),
    /// The branch changed without the developer, because the base branch
    /// moved and was merged in. Everything after the developer step must run
    /// again on the new commit.
    Again(String),
    /// It failed in a way no fix round can help.
    Stop(String),
}

/// Why the developer is being called again.
#[derive(Debug, Clone, PartialEq)]
pub struct Feedback {
    /// The step that failed.
    pub from: String,
    pub reason: String,
    /// Which fix round this is within its budget, and that budget's size.
    pub round: u32,
    pub limit: u32,
}

/// What a step is told about where the run stands.
pub struct Turn<'a> {
    /// How many times the run has gone back so far, for any reason. 0 on the
    /// first pass. It names files and log rows, so nothing is overwritten.
    pub pass: u32,
    /// Set only for the developer step of a fix round.
    pub feedback: Option<&'a Feedback>,
}

/// The two budgets of fix rounds. Failures found on GitHub, after the pull
/// request is open, draw on their own.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub local: u32,
    pub github: u32,
    /// The first step whose failures draw on the GitHub budget. Past the end
    /// when the run has no GitHub steps.
    pub github_from: usize,
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum End {
    Passed,
    /// A step failed and nothing may retry it.
    Blocked { step: String, reason: String },
    /// A step failed with every fix round of its budget already used.
    OutOfRounds { step: String, reason: String, limit: u32, github: bool },
}

/// How many times the run went back, by cause.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Used {
    pub local: u32,
    pub github: u32,
    pub syncs: u32,
}

impl Used {
    pub fn passes(&self) -> u32 {
        self.local + self.github + self.syncs
    }
}

/// Does one step's work.
pub type RunStep<'a> = dyn FnMut(usize, &Turn) -> Result<Outcome> + 'a;

/// Runs the steps in order. A step that asks for rework sends the run back to
/// the developer step, which then runs again with the reason, followed by
/// every step after it. A step that says the branch changed sends the run
/// back to the step after the developer.
pub fn drive(names: &[String], dev: usize, on_fail: &BTreeMap<String, String>, limits: Limits, run: &mut RunStep) -> Result<(End, Used)> {
    let mut index = 0;
    let mut used = Used::default();
    let mut feedback: Option<Feedback> = None;
    while index < names.len() {
        let name = &names[index];
        let turn = Turn { pass: used.passes(), feedback: if index == dev { feedback.as_ref() } else { None } };
        let reason = match run(index, &turn)? {
            Outcome::Pass => {
                if index == dev {
                    feedback = None;
                }
                index += 1;
                continue;
            }
            Outcome::Stop(reason) => return Ok((End::Blocked { step: name.clone(), reason }, used)),
            Outcome::Again(reason) => {
                if used.syncs >= MAX_SYNCS {
                    let reason = format!("{reason}\n(the base branch moved {MAX_SYNCS} times already during this run)");
                    return Ok((End::Blocked { step: name.clone(), reason }, used));
                }
                used.syncs += 1;
                index = dev + 1;
                continue;
            }
            Outcome::Rework(reason) => reason,
        };
        // The developer's own gates send it back to itself. Any other step
        // goes where the workflow file says.
        let target = if index == dev { Some(name) } else { on_fail.get(name) };
        match target {
            Some(target) if *target == names[dev] && dev <= index => {}
            Some(target) => {
                let reason = format!("{reason}\n(on_fail sends '{name}' to '{target}', and only a return to the developer step is built)");
                return Ok((End::Blocked { step: name.clone(), reason }, used));
            }
            None => {
                let reason = format!("{reason}\n(the workflow file has no on_fail for '{name}')");
                return Ok((End::Blocked { step: name.clone(), reason }, used));
            }
        }
        let github = index >= limits.github_from;
        let (spent, limit) = if github { (&mut used.github, limits.github) } else { (&mut used.local, limits.local) };
        if *spent >= limit {
            return Ok((End::OutOfRounds { step: name.clone(), reason, limit, github }, used));
        }
        *spent += 1;
        feedback = Some(Feedback { from: name.clone(), reason, round: *spent, limit });
        index = dev;
    }
    Ok((End::Passed, used))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        ["acceptance", "red", "develop", "verify", "review", "sync", "pr", "github", "merge"].iter().map(|s| s.to_string()).collect()
    }

    fn on_fail() -> BTreeMap<String, String> {
        [("red", "acceptance"), ("verify", "develop"), ("review", "develop"), ("github", "develop")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    fn limits(local: u32, github: u32) -> Limits {
        Limits { local, github, github_from: 7 }
    }

    /// Plays a script: each step name maps to the outcomes of its successive
    /// calls. A step with no outcome left passes. Returns the result and
    /// every call as `name@pass`, with `!step` when feedback was given.
    fn play(limits: Limits, script: &[(&str, Vec<Outcome>)]) -> ((End, Used), Vec<String>) {
        let names = names();
        let mut left: BTreeMap<String, Vec<Outcome>> =
            script.iter().map(|(name, outcomes)| (name.to_string(), outcomes.iter().rev().cloned().collect())).collect();
        let mut calls = Vec::new();
        let result = drive(&names, 2, &on_fail(), limits, &mut |index, turn| {
            let name = &names[index];
            calls.push(format!("{name}@{}{}", turn.pass, turn.feedback.map(|f| format!("!{}", f.from)).unwrap_or_default()));
            Ok(left.get_mut(name).and_then(Vec::pop).unwrap_or(Outcome::Pass))
        })
        .unwrap();
        (result, calls)
    }

    fn rework(text: &str) -> Outcome {
        Outcome::Rework(text.to_string())
    }

    fn used(local: u32, github: u32, syncs: u32) -> Used {
        Used { local, github, syncs }
    }

    #[test]
    fn a_clean_run_calls_each_step_once() {
        let (result, calls) = play(limits(3, 2), &[]);
        assert_eq!(result, (End::Passed, used(0, 0, 0)));
        assert_eq!(calls, vec!["acceptance@0", "red@0", "develop@0", "verify@0", "review@0", "sync@0", "pr@0", "github@0", "merge@0"]);
    }

    #[test]
    fn a_failed_verify_returns_to_the_developer_and_skips_review() {
        let (result, calls) = play(limits(3, 2), &[("verify", vec![rework("flow failed")])]);
        assert_eq!(result.1, used(1, 0, 0));
        assert_eq!(&calls[2..7], ["develop@0", "verify@0", "develop@1!verify", "verify@1", "review@1"]);
    }

    #[test]
    fn a_review_asking_for_changes_is_verified_again_before_the_next_review() {
        let (result, calls) = play(limits(3, 2), &[("review", vec![rework("R1")])]);
        assert_eq!(result, (End::Passed, used(1, 0, 0)));
        assert_eq!(&calls[4..8], ["review@0", "develop@1!review", "verify@1", "review@1"]);
    }

    #[test]
    fn rounds_run_out() {
        let always = vec![rework("a"), rework("b"), rework("c"), rework("d"), rework("e")];
        let (result, calls) = play(limits(3, 2), &[("verify", always.clone())]);
        assert_eq!(result, (End::OutOfRounds { step: "verify".into(), reason: "d".into(), limit: 3, github: false }, used(3, 0, 0)));
        assert_eq!(calls.iter().filter(|c| c.starts_with("develop")).count(), 4);

        let (result, calls) = play(limits(0, 2), &[("verify", always)]);
        assert_eq!(result.0, End::OutOfRounds { step: "verify".into(), reason: "a".into(), limit: 0, github: false });
        assert_eq!(calls.iter().filter(|c| c.starts_with("develop")).count(), 1);
    }

    #[test]
    fn local_rounds_are_shared_between_verify_and_review() {
        let script = [("verify", vec![rework("v1"), rework("v2")]), ("review", vec![rework("r1"), rework("r2")])];
        let (result, _) = play(limits(3, 2), &script);
        assert_eq!(result.0, End::OutOfRounds { step: "review".into(), reason: "r2".into(), limit: 3, github: false });
    }

    #[test]
    fn github_failures_have_their_own_budget() {
        // A failure on GitHub goes back to the developer, and everything
        // local runs again before the next push.
        let (result, calls) = play(limits(0, 2), &[("github", vec![rework("ci red")])]);
        assert_eq!(result, (End::Passed, used(0, 1, 0)));
        assert_eq!(&calls[7..], ["github@0", "develop@1!github", "verify@1", "review@1", "sync@1", "pr@1", "github@1", "merge@1"]);

        let always = vec![rework("a"), rework("b"), rework("c")];
        let (result, _) = play(limits(3, 2), &[("github", always)]);
        assert_eq!(result, (End::OutOfRounds { step: "github".into(), reason: "c".into(), limit: 2, github: true }, used(0, 2, 0)));

        // Local rounds spent after a GitHub round still count as local.
        let script = [("github", vec![rework("ci red")]), ("verify", vec![Outcome::Pass, rework("v")])];
        let (result, _) = play(limits(3, 2), &script);
        assert_eq!(result, (End::Passed, used(1, 1, 0)));
    }

    #[test]
    fn a_moved_base_branch_reruns_everything_after_the_developer() {
        let (result, calls) = play(limits(3, 2), &[("merge", vec![Outcome::Again("dev moved".into())])]);
        assert_eq!(result, (End::Passed, used(0, 0, 1)));
        assert_eq!(&calls[8..], ["merge@0", "verify@1", "review@1", "sync@1", "pr@1", "github@1", "merge@1"]);
        assert!(!calls.iter().any(|c| c.starts_with("develop@1")), "the developer is not called for a sync");

        let forever = vec![Outcome::Again("moved".into()); 9];
        let (result, _) = play(limits(3, 2), &[("sync", forever)]);
        let (End::Blocked { step, reason }, counts) = result else { panic!("{result:?}") };
        assert_eq!((step.as_str(), counts), ("sync", used(0, 0, 3)));
        assert!(reason.contains("moved 3 times already"), "{reason}");
    }

    #[test]
    fn feedback_carries_the_round_within_its_budget() {
        let names = names();
        let mut seen = Vec::new();
        let mut github_failures = 1;
        let mut verify_failures = 1;
        drive(&names, 2, &on_fail(), limits(3, 2), &mut |index, turn| {
            if let Some(feedback) = turn.feedback {
                seen.push((feedback.from.clone(), feedback.round, feedback.limit, turn.pass));
            }
            Ok(match names[index].as_str() {
                "verify" if verify_failures > 0 => {
                    verify_failures -= 1;
                    rework("v")
                }
                "github" if github_failures > 0 => {
                    github_failures -= 1;
                    rework("g")
                }
                _ => Outcome::Pass,
            })
        })
        .unwrap();
        assert_eq!(seen, vec![("verify".to_string(), 1, 3, 1), ("github".to_string(), 1, 2, 2)]);
    }

    #[test]
    fn the_developers_own_gates_send_it_back_to_itself() {
        let (result, calls) = play(limits(3, 2), &[("develop", vec![rework("outside scope")])]);
        assert_eq!(result, (End::Passed, used(1, 0, 0)));
        assert_eq!(&calls[2..6], ["develop@0", "develop@1!develop", "verify@1", "review@1"]);
    }

    #[test]
    fn a_stop_is_never_retried() {
        let (result, calls) = play(limits(3, 2), &[("develop", vec![Outcome::Stop("changed no files".into())])]);
        assert_eq!(result, (End::Blocked { step: "develop".into(), reason: "changed no files".into() }, used(0, 0, 0)));
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn a_failure_with_no_way_back_to_the_developer_blocks() {
        // red returns to acceptance, which is before the developer.
        let (result, calls) = play(limits(3, 2), &[("red", vec![rework("the flow already passes")])]);
        let (End::Blocked { step, reason }, _) = result else { panic!("{result:?}") };
        assert_eq!(step, "red");
        assert!(reason.contains("only a return to the developer step is built"), "{reason}");
        assert_eq!(calls, vec!["acceptance@0", "red@0"]);

        let names = names();
        let (end, _) = drive(&names, 2, &BTreeMap::new(), limits(3, 2), &mut |index, _| {
            Ok(if names[index] == "verify" { rework("x") } else { Outcome::Pass })
        })
        .unwrap();
        let End::Blocked { reason, .. } = end else { panic!() };
        assert!(reason.contains("no on_fail for 'verify'"), "{reason}");
    }
}
