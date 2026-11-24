//! The fix loop, as plain logic: which step runs next, when a failure goes
//! back to the developer, and when the rounds run out.
//!
//! Nothing here calls an agent or touches git, so every path can be tested.

use anyhow::Result;
use std::collections::BTreeMap;

/// Most fix rounds a workflow file may ask for. The limit can be lowered, and
/// raised up to this, but never removed.
pub const MAX_FIX_ROUNDS: u32 = 5;
pub const DEFAULT_FIX_ROUNDS: u32 = 3;

/// How one step ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pass,
    /// It failed in a way the developer can fix. The text is what the
    /// developer is told.
    Rework(String),
    /// It failed in a way no fix round can help.
    Stop(String),
}

/// Why the developer is being called again.
#[derive(Debug, Clone, PartialEq)]
pub struct Feedback {
    /// The step that failed.
    pub from: String,
    pub reason: String,
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum End {
    Passed,
    /// A step failed and nothing may retry it.
    Blocked { step: String, reason: String },
    /// A step failed with every fix round already used.
    OutOfRounds { step: String, reason: String },
}

/// Does one step's work: `(step index, round, feedback for the developer)`.
pub type RunStep<'a> = dyn FnMut(usize, u32, Option<&Feedback>) -> Result<Outcome> + 'a;

/// Runs the steps in order. A step that asks for rework sends the run back to
/// the developer step, which then runs again with the reason, followed by
/// every step after it. `run(step, round, feedback)` does the work: `round`
/// is 0 for the first pass and counts fix rounds after that, and `feedback`
/// is set only for the developer step of a fix round.
///
/// Returns how the run ended and how many fix rounds it used.
pub fn drive(
    names: &[String],
    dev: usize,
    on_fail: &BTreeMap<String, String>,
    limit: u32,
    run: &mut RunStep,
) -> Result<(End, u32)> {
    let mut index = 0;
    let mut round = 0;
    let mut feedback: Option<Feedback> = None;
    while index < names.len() {
        let name = &names[index];
        let given = if index == dev { feedback.as_ref() } else { None };
        let reason = match run(index, round, given)? {
            Outcome::Pass => {
                if index == dev {
                    feedback = None;
                }
                index += 1;
                continue;
            }
            Outcome::Stop(reason) => return Ok((End::Blocked { step: name.clone(), reason }, round)),
            Outcome::Rework(reason) => reason,
        };
        // The developer's own gates send it back to itself. Any other step
        // goes where the workflow file says.
        let target = if index == dev { Some(name) } else { on_fail.get(name) };
        match target {
            Some(target) if *target == names[dev] && dev <= index => {}
            Some(target) => {
                let reason = format!("{reason}\n(on_fail sends '{name}' to '{target}', and only a return to the developer step is built)");
                return Ok((End::Blocked { step: name.clone(), reason }, round));
            }
            None => {
                let reason = format!("{reason}\n(the workflow file has no on_fail for '{name}')");
                return Ok((End::Blocked { step: name.clone(), reason }, round));
            }
        }
        if round >= limit {
            return Ok((End::OutOfRounds { step: name.clone(), reason }, round));
        }
        round += 1;
        feedback = Some(Feedback { from: name.clone(), reason });
        index = dev;
    }
    Ok((End::Passed, round))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        ["acceptance", "red", "develop", "verify", "review"].iter().map(|s| s.to_string()).collect()
    }

    fn on_fail() -> BTreeMap<String, String> {
        [("red", "acceptance"), ("verify", "develop"), ("review", "develop")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    /// Plays a script: each step name maps to the outcomes of its successive
    /// calls. A step with no outcome left passes. Returns the result and
    /// every call as `name@round` with `!` when feedback was given.
    fn play(limit: u32, script: &[(&str, Vec<Outcome>)]) -> ((End, u32), Vec<String>) {
        let names = names();
        let mut left: BTreeMap<String, Vec<Outcome>> =
            script.iter().map(|(name, outcomes)| (name.to_string(), outcomes.iter().rev().cloned().collect())).collect();
        let mut calls = Vec::new();
        let result = drive(&names, 2, &on_fail(), limit, &mut |index, round, feedback| {
            let name = &names[index];
            calls.push(format!("{name}@{round}{}", feedback.map(|f| format!("!{}", f.from)).unwrap_or_default()));
            Ok(left.get_mut(name).and_then(Vec::pop).unwrap_or(Outcome::Pass))
        })
        .unwrap();
        (result, calls)
    }

    fn rework(text: &str) -> Outcome {
        Outcome::Rework(text.to_string())
    }

    #[test]
    fn a_clean_run_calls_each_step_once() {
        let (result, calls) = play(3, &[]);
        assert_eq!(result, (End::Passed, 0));
        assert_eq!(calls, vec!["acceptance@0", "red@0", "develop@0", "verify@0", "review@0"]);
    }

    #[test]
    fn a_failed_verify_returns_to_the_developer_and_skips_review() {
        let (result, calls) = play(3, &[("verify", vec![rework("flow failed")])]);
        assert_eq!(result, (End::Passed, 1));
        assert_eq!(calls, vec!["acceptance@0", "red@0", "develop@0", "verify@0", "develop@1!verify", "verify@1", "review@1"]);
    }

    #[test]
    fn a_review_asking_for_changes_is_verified_again_before_the_next_review() {
        let (result, calls) = play(3, &[("review", vec![rework("R1")])]);
        assert_eq!(result, (End::Passed, 1));
        assert_eq!(&calls[4..], ["review@0", "develop@1!review", "verify@1", "review@1"]);
    }

    #[test]
    fn rounds_run_out() {
        let always = vec![rework("a"), rework("b"), rework("c"), rework("d"), rework("e")];
        let (result, calls) = play(3, &[("verify", always.clone())]);
        assert_eq!(result, (End::OutOfRounds { step: "verify".into(), reason: "d".into() }, 3));
        assert_eq!(calls.iter().filter(|c| c.starts_with("develop")).count(), 4);

        let (result, calls) = play(0, &[("verify", always)]);
        assert_eq!(result, (End::OutOfRounds { step: "verify".into(), reason: "a".into() }, 0));
        assert_eq!(calls.iter().filter(|c| c.starts_with("develop")).count(), 1);
    }

    #[test]
    fn rounds_are_shared_between_verify_and_review() {
        let script = [("verify", vec![rework("v1"), rework("v2")]), ("review", vec![rework("r1"), rework("r2")])];
        let (result, _) = play(3, &script);
        assert_eq!(result, (End::OutOfRounds { step: "review".into(), reason: "r2".into() }, 3));
    }

    #[test]
    fn the_developers_own_gates_send_it_back_to_itself() {
        let (result, calls) = play(3, &[("develop", vec![rework("outside scope")])]);
        assert_eq!(result, (End::Passed, 1));
        assert_eq!(&calls[2..], ["develop@0", "develop@1!develop", "verify@1", "review@1"]);
    }

    #[test]
    fn a_stop_is_never_retried() {
        let (result, calls) = play(3, &[("develop", vec![Outcome::Stop("changed no files".into())])]);
        assert_eq!(result, (End::Blocked { step: "develop".into(), reason: "changed no files".into() }, 0));
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn a_failure_with_no_way_back_to_the_developer_blocks() {
        // red returns to acceptance, which is before the developer.
        let (result, calls) = play(3, &[("red", vec![rework("the flow already passes")])]);
        let (End::Blocked { step, reason }, 0) = result else { panic!("{result:?}") };
        assert_eq!(step, "red");
        assert!(reason.contains("only a return to the developer step is built"), "{reason}");
        assert_eq!(calls, vec!["acceptance@0", "red@0"]);

        let names = names();
        let (end, _) = drive(&names, 2, &BTreeMap::new(), 3, &mut |index, _, _| {
            Ok(if names[index] == "verify" { rework("x") } else { Outcome::Pass })
        })
        .unwrap();
        let End::Blocked { reason, .. } = end else { panic!() };
        assert!(reason.contains("no on_fail for 'verify'"), "{reason}");
    }
}
