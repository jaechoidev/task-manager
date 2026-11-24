//! Runs the real `factory run` against a local git repository, with small
//! scripts standing in for `gh`, `codex` and `claude`. Git, the worktree, the
//! gates, the commits, `verify.sh` and the state files are all real.
//!
//! These cover the paths a real run rarely takes: a verify that fails, a
//! review that asks for changes, a gate that rejects the developer's change,
//! and rounds that run out.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GH: &str = r#"#!/bin/bash
# Stands in for gh: one repository, o/r, with one open issue, #7.
cat > /dev/null
if [ "$1" = "repo" ]; then echo "o/r"; exit 0; fi
if [ "$1" != "api" ]; then exit 1; fi
case "$3 $6" in
  "GET repos/o/r/issues/7") cat "$STUB_DIR/issue.json" ;;
  "GET repos/o/r/issues/7/"*) echo "[]" ;;
  *) echo "{}" ;;
esac
"#;

const CODEX: &str = r#"#!/bin/bash
# Stands in for the developer. What it writes depends on STUB_DEV.
n=$(( $(cat "$STUB_DIR/codex.count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$STUB_DIR/codex.count"
echo "$*" > "$STUB_DIR/codex.args.$n"
cat > "$STUB_DIR/codex.prompt.$n"
out=""; prev=""
for arg in "$@"; do [ "$prev" = "-o" ] && out="$arg"; prev="$arg"; done
mkdir -p Sources
case "$STUB_DEV" in
  fix-on-second)
    if [ "$n" = 1 ]; then echo "let a = 1" > Sources/A.swift; else echo "// FIXED" >> Sources/A.swift; fi ;;
  right-first-time)
    if [ "$n" = 1 ]; then printf 'let a = 1\n// FIXED\n' > Sources/A.swift; else echo "let renamed = 1" >> Sources/A.swift; fi ;;
  never)
    echo "// attempt $n" >> Sources/A.swift ;;
  outside-scope-first)
    printf 'let a = 1\n// FIXED\n' > Sources/A.swift
    if [ "$n" = 1 ]; then mkdir -p Other; echo x > Other/X.swift; else rm -rf Other; fi ;;
  asks-for-a-flow-change)
    if [ "$n" = 1 ]; then
      printf 'let id = "a.b"\n// FIXED\n' > Sources/A.swift
      request='"a.yaml: drop the last assertion, the criteria do not ask for it"'
    else
      echo "// meets the flow as written" >> Sources/A.swift
    fi ;;
esac
echo '{"thread_id":"stub-session"}'
echo "{\"summary\":\"attempt $n\",\"commit_subject\":\"feat(a): attempt $n\",\"files_changed\":[\"Sources/A.swift\"],\"verify_ran\":\"not_run\",\"notes\":\"\",\"flow_change_request\":${request:-null}}" > "$out"
"#;

const CLAUDE: &str = r#"#!/bin/bash
# Stands in for Claude in whichever role the prompt gives it: the acceptance
# writer, the same writer ruling on a flow change request, or the reviewer.
n=$(( $(cat "$STUB_DIR/claude.count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$STUB_DIR/claude.count"
echo "$*" > "$STUB_DIR/claude.args.$n"
cat > "$STUB_DIR/claude.prompt.$n"
reply() { echo "{\"type\":\"result\",\"is_error\":false,\"session_id\":\"stub-claude-$n\",\"structured_output\":$1}"; }
if grep -q "Rule on that request" "$STUB_DIR/claude.prompt.$n"; then
  echo ruling >> "$STUB_DIR/claude.roles"
  if [ "$STUB_RULING" = accept ]; then
    printf '%s\n' '- assertVisible:' '    id: "a.b"' >> .maestro/a.yaml
    reply '{"decision":"accept","reason":"the flow asserted more than the criteria ask"}'
  else
    reply '{"decision":"reject","reason":"the criteria require that assertion"}'
  fi
  exit 0
fi
if grep -q "You are the acceptance writer" "$STUB_DIR/claude.prompt.$n"; then
  echo acceptance >> "$STUB_DIR/claude.roles"
  mkdir -p .maestro
  printf '%s\n' 'appId: com.example.trial' '---' '- launchApp' '- tapOn:' '    id: "a.b"' > .maestro/a.yaml
  reply '{"summary":"taps the element","flows":[".maestro/a.yaml"],"added_ids":[],"notes":"the element must be tappable"}'
  exit 0
fi
echo review >> "$STUB_DIR/claude.roles"
sha=$(sed -n 's/.*Commit under review: `\([0-9a-f]*\)`.*/\1/p' "$STUB_DIR/claude.prompt.$n" | head -1)
verdict=approve; items='[]'
if [ "$STUB_REVIEW" = "changes-once" ] && [ "$n" = 1 ]; then
  verdict=changes; items='["Sources/A.swift: rename the constant"]'
fi
echo "{\"type\":\"result\",\"is_error\":false,\"session_id\":\"stub-review\",\"structured_output\":{\"verdict\":\"$verdict\",\"head_sha\":\"$sha\",\"summary\":\"looked at it\",\"items\":$items}}"
"#;

const VERIFY: &str = r#"#!/bin/sh
# Passes only when the developer's file carries the marker. In red mode it
# reports that the named flows fail, as they do before development.
if [ "$1" = "--red" ]; then echo "red: $*"; exit 0; fi
if grep -q FIXED Sources/A.swift 2>/dev/null; then echo "all good"; exit 0; fi
echo "Sources/A.swift lacks the FIXED marker"
exit 1
"#;

const ISSUE_WITH_FLOW: &str = "<!-- factory:issue {\"v\":1,\"plan\":\"trial\",\"key\":\"a\",\"scope\":[\"Sources/\"],\"accept\":[\"The element can be tapped\"],\"flows\":[{\"file\":\"a.yaml\",\"new\":true}],\"accessibility_ids\":[\"a.b\"]} -->\n\nAdd the element.\n";

const WORKFLOW_WITH_ACCEPTANCE: &str = "workflow:
  - acceptance: { module: agent/acceptance, vendor: claude }
  - red:        { module: check/acceptance-red }
  - develop:    { module: agent/dev, vendor: codex }
  - verify:     { module: check/verify }
  - review:     { module: agent/review, vendor: claude }
on_fail:
  red: acceptance
  verify: develop
  review: develop
";

const ISSUE_BODY: &str = "<!-- factory:issue {\"v\":1,\"plan\":\"trial\",\"key\":\"a\",\"scope\":[\"Sources/\"],\"accept\":[\"A.swift carries the marker\"]} -->\n\nAdd the file.\n\n## Scope\n\n- `Sources/`\n";

struct Trial {
    dir: PathBuf,
}

impl Trial {
    /// A repository with a remote, a `dev` default branch and a workflow that
    /// allows `fix_rounds` rounds.
    fn new(name: &str, fix_rounds: u32) -> Trial {
        let workflow = format!(
            "workflow:\n  - develop: {{ module: agent/dev, vendor: codex }}\n  - verify:  {{ module: check/verify }}\n  - review:  {{ module: agent/review, vendor: claude }}\non_fail:\n  verify: develop\n  review: develop\nlimits:\n  fix_rounds: {fix_rounds}\n"
        );
        Trial::build(name, &workflow, ISSUE_BODY)
    }

    fn build(name: &str, workflow: &str, issue_body: &str) -> Trial {
        let dir = std::env::temp_dir().join(format!("factory-loop-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let trial = Trial { dir };
        for sub in ["bin", "stub", "home", "repo"] {
            fs::create_dir_all(trial.dir.join(sub)).unwrap();
        }
        fs::write(trial.dir.join("gitconfig"), "[user]\n\tname = Trial\n\temail = trial@example.invalid\n").unwrap();
        for (file, text) in [("gh", GH), ("codex", CODEX), ("claude", CLAUDE)] {
            executable(&trial.dir.join("bin").join(file), text);
        }
        let issue = serde_json::json!({ "number": 7, "state": "open", "title": "Add the file", "body": issue_body });
        fs::write(trial.dir.join("stub/issue.json"), issue.to_string()).unwrap();

        let repo = trial.dir.join("repo");
        fs::write(repo.join("factory.yaml"), workflow).unwrap();
        executable(&repo.join("verify.sh"), VERIFY);
        let origin = trial.dir.join("origin.git");
        trial.git(&trial.dir, &["init", "--quiet", "--bare", "-b", "dev", origin.to_str().unwrap()]);
        trial.git(&repo, &["init", "--quiet", "-b", "dev"]);
        trial.git(&repo, &["add", "-A"]);
        trial.git(&repo, &["commit", "--quiet", "-m", "start"]);
        trial.git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        trial.git(&repo, &["push", "--quiet", "-u", "origin", "dev"]);
        trial.git(&repo, &["remote", "set-head", "origin", "dev"]);
        trial
    }

    fn command(&self, program: &str) -> Command {
        let path = format!("{}:{}", self.dir.join("bin").display(), std::env::var("PATH").unwrap());
        let mut command = Command::new(program);
        command
            .env("PATH", path)
            .env("FACTORY_HOME", self.dir.join("home"))
            .env("STUB_DIR", self.dir.join("stub"))
            .env("GIT_CONFIG_GLOBAL", self.dir.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1");
        command
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.command("git").args(args).current_dir(cwd).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Runs `factory run 7 --no-pr` with the given stand-in behavior.
    fn run(&self, dev: &str, review: &str) -> (Output, String) {
        self.run_ruling(dev, review, "")
    }

    fn run_ruling(&self, dev: &str, review: &str, ruling: &str) -> (Output, String) {
        let output = self
            .command(env!("CARGO_BIN_EXE_factory"))
            .args(["run", "7", "--no-pr", "--repo-dir"])
            .arg(self.dir.join("repo"))
            .env("STUB_DEV", dev)
            .env("STUB_REVIEW", review)
            .env("STUB_RULING", ruling)
            .output()
            .unwrap();
        let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        (output, text)
    }

    fn stub(&self, file: &str) -> String {
        fs::read_to_string(self.dir.join("stub").join(file)).unwrap_or_default().trim().to_string()
    }

    /// Subjects of the commits the run added, oldest first.
    fn commits(&self) -> Vec<String> {
        let worktree = self.dir.join("home/worktrees/o__r/issue-7");
        self.git(&worktree, &["log", "--reverse", "--format=%s", "origin/dev..HEAD"]).lines().map(str::to_string).collect()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("home/requests.jsonl")).unwrap_or_default()
    }
}

impl Drop for Trial {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_failed_verify_goes_back_to_the_developer_with_the_log() {
    let trial = Trial::new("verify", 3);
    let (output, text) = trial.run("fix-on-second", "approve");
    assert!(output.status.success(), "{text}");
    assert!(text.contains("passed after 1 fix round(s)"), "{text}");
    assert_eq!(trial.commits(), vec!["feat(a): attempt 1", "feat(a): attempt 2"]);

    // The developer was called twice. The second call continued its session,
    // named its sandbox again, and carried what verify printed.
    assert_eq!(trial.stub("codex.count"), "2");
    assert!(trial.stub("codex.args.1").starts_with("exec --cd "), "{}", trial.stub("codex.args.1"));
    let second = trial.stub("codex.args.2");
    assert!(second.starts_with("exec resume stub-session -c sandbox_mode=\"workspace-write\""), "{second}");
    let prompt = trial.stub("codex.prompt.2");
    assert!(prompt.contains("fix round 1 of 3"), "{prompt}");
    assert!(prompt.contains("at the `verify` step"), "{prompt}");
    assert!(prompt.contains("Sources/A.swift lacks the FIXED marker"), "{prompt}");
    assert!(prompt.contains("- `Sources/`"), "the fix prompt must restate the scope: {prompt}");

    // The reviewer only ever saw the commit that passed verify.
    assert_eq!(trial.stub("claude.count"), "1");
    let log = trial.log();
    assert!(log.contains("\"step\":\"verify\"") && log.contains("\"round\":1"), "{log}");
    assert_eq!(log.matches("\"step\":\"verify\"").count(), 2);
}

#[test]
fn a_review_asking_for_changes_is_followed_by_verify_and_a_fresh_review() {
    let trial = Trial::new("review", 3);
    let (output, text) = trial.run("right-first-time", "changes-once");
    assert!(output.status.success(), "{text}");
    assert!(text.contains("passed after 1 fix round(s)"), "{text}");
    assert_eq!(trial.commits().len(), 2);

    let prompt = trial.stub("codex.prompt.2");
    assert!(prompt.contains("at the `review` step"), "{prompt}");
    assert!(prompt.contains("R1: Sources/A.swift: rename the constant"), "{prompt}");

    // Verify ran again on the new commit, and the second review was told
    // what the first one asked for.
    assert_eq!(trial.log().matches("\"step\":\"verify\"").count(), 2);
    assert_eq!(trial.stub("claude.count"), "2");
    let first = trial.stub("claude.prompt.1");
    let second = trial.stub("claude.prompt.2");
    assert!(first.contains("None. This is the first review."), "{first}");
    assert!(second.contains("Sources/A.swift: rename the constant"), "{second}");
    assert!(second.contains("An earlier commit"), "{second}");
    assert!(!trial.stub("claude.args.2").contains("--resume"), "each review is a new session");
}

#[test]
fn rounds_run_out_and_the_issue_ends_blocked() {
    let trial = Trial::new("limit", 1);
    let (output, text) = trial.run("never", "approve");
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("1 fix round was used and 'verify' still fails"), "{text}");
    assert!(text.contains("Sources/A.swift lacks the FIXED marker"), "{text}");
    // One first attempt and one fix round, then no more calls. No review.
    assert_eq!(trial.stub("codex.count"), "2");
    assert_eq!(trial.stub("claude.count"), "");
    assert_eq!(trial.commits().len(), 2);
}

#[test]
fn a_change_outside_scope_is_sent_back_before_anything_is_committed() {
    let trial = Trial::new("scope", 3);
    let (output, text) = trial.run("outside-scope-first", "approve");
    assert!(output.status.success(), "{text}");
    assert!(text.contains("passed after 1 fix round(s)"), "{text}");
    // The rejected attempt was never committed.
    assert_eq!(trial.commits(), vec!["feat(a): attempt 2"]);
    let prompt = trial.stub("codex.prompt.2");
    assert!(prompt.contains("at the `develop` step"), "{prompt}");
    assert!(prompt.contains("outside the issue's scope: Other/X.swift"), "{prompt}");
    assert!(prompt.contains("Nothing of yours is committed yet"), "{prompt}");
    // Verify never ran on the rejected attempt.
    assert_eq!(trial.log().matches("\"step\":\"verify\"").count(), 1);
}

#[test]
fn an_accepted_flow_change_is_its_own_commit_and_is_what_gets_judged() {
    let trial = Trial::build("accept", WORKFLOW_WITH_ACCEPTANCE, ISSUE_WITH_FLOW);
    let (output, text) = trial.run_ruling("asks-for-a-flow-change", "approve", "accept");
    assert!(output.status.success(), "{text}");
    assert!(!text.contains("fix round"), "an accepted request needs no fix round: {text}");
    assert_eq!(
        trial.commits(),
        vec!["test(acceptance): flows for #7", "feat(a): attempt 1", "test(acceptance): accept flow change for #7"]
    );
    assert_eq!(trial.stub("claude.roles"), "acceptance\nruling\nreview");
    assert_eq!(trial.stub("codex.count"), "1");

    // The writer was shown the request, and the reviewer judged the commit
    // that holds the changed flow, knowing the flow was changed.
    let ruling = trial.stub("claude.prompt.2");
    assert!(ruling.contains("a.yaml: drop the last assertion"), "{ruling}");
    let worktree = trial.dir.join("home/worktrees/o__r/issue-7");
    let head = trial.git(&worktree, &["rev-parse", "HEAD"]);
    let review = trial.stub("claude.prompt.3");
    assert!(review.contains(&format!("Commit under review: `{head}`")), "{review}");
    assert!(review.contains("accepted a flow change request"), "{review}");
    // The developer saw the writer's notes and the red check ran on the new flow.
    assert!(trial.stub("codex.prompt.1").contains("the element must be tappable"));
    assert!(text.contains("red: running ./verify.sh --red '.maestro/a.yaml'"), "{text}");
    assert!(trial.log().contains("\"step\":\"flow-ruling\""));
}

#[test]
fn a_rejected_flow_change_sends_the_developer_back_to_the_code() {
    let trial = Trial::build("reject", WORKFLOW_WITH_ACCEPTANCE, ISSUE_WITH_FLOW);
    let (output, text) = trial.run_ruling("asks-for-a-flow-change", "approve", "reject");
    assert!(output.status.success(), "{text}");
    assert!(text.contains("passed after 1 fix round(s)"), "{text}");
    assert_eq!(trial.commits(), vec!["test(acceptance): flows for #7", "feat(a): attempt 1", "feat(a): attempt 2"]);
    // The flow is untouched since the writer's commit.
    let worktree = trial.dir.join("home/worktrees/o__r/issue-7");
    assert_eq!(trial.git(&worktree, &["log", "--format=%s", "origin/dev..HEAD", "--", ".maestro"]), "test(acceptance): flows for #7");
    assert_eq!(trial.stub("claude.roles"), "acceptance\nruling\nreview");
    let prompt = trial.stub("codex.prompt.2");
    assert!(prompt.contains("rejected your flow change request"), "{prompt}");
    assert!(prompt.contains("the criteria require that assertion"), "{prompt}");
    assert!(prompt.contains("at the `develop` step"), "{prompt}");
}
