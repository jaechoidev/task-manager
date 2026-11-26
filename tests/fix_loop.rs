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

const GH: &str = r#"#!/usr/bin/env python3
# Stands in for gh and for GitHub: one repository, o/r. With issue.json it
# has one open issue, #7. With issues.json it has several. The pull request
# for issue N is number N + 2. STUB_GITHUB says how checks and reviews answer.
# A second repository, o/r2, exists when a test made the checkout `repo2`:
# its state is in the folder r2 and its remote is origin2.git.
import glob, json, os, re, subprocess, sys

stub = os.environ["STUB_DIR"]
mode = os.environ.get("STUB_GITHUB", "green")
origin = f"{stub}/../origin.git"
body = sys.stdin.read()
args = sys.argv[1:]
if args[:1] == ["repo"]:
    print("o/r2" if os.path.basename(os.getcwd()) == "repo2" else "o/r")
    sys.exit(0)
if args[:1] != ["api"]:
    sys.exit(1)
method = args[2]
path = args[5] if args[3] == "-H" else args[3]
if path.startswith("repos/o/r2"):
    # The second repository answers like the first, from its own state.
    origin = f"{stub}/../origin2.git"
    stub = f"{stub}/r2"
    path = path.replace("repos/o/r2", "repos/o/r", 1)
with open(f"{stub}/gh.calls", "a") as calls:
    calls.write(f"{method} {path}\n")

def out(value):
    print(json.dumps(value))

def save(name, number, text):
    # Under a name for the issue, and under the plain name single-issue tests read.
    open(f"{stub}/{name}-{number}.json", "w").write(text)
    open(f"{stub}/{name}.json", "w").write(text)

def issues():
    if os.path.exists(f"{stub}/issues.json"):
        return json.load(open(f"{stub}/issues.json"))
    return {"7": json.load(open(f"{stub}/issue.json"))}

def issue(number):
    found = dict(issues()[str(number)])
    found["number"] = int(number)
    found["state"] = "closed" if os.path.exists(f"{stub}/closed-{number}") else "open"
    found["labels"] = [{"name": name} for name in found.get("labels", [])]
    return found

def git(*command):
    return subprocess.run(["git", "--git-dir", origin, *command], capture_output=True, text=True).stdout.strip()

def branch_of(number):
    refs = git("for-each-ref", "--format=%(refname:short)", f"refs/heads/factory/issue-{number}-*")
    return refs.splitlines()[0] if refs else ""

def nth(sha):
    # 1 for the first commit of a pull request GitHub was asked about.
    file = f"{stub}/gh.shas"
    seen = open(file).read().split() if os.path.exists(file) else []
    if sha not in seen:
        seen.append(sha)
        open(file, "w").write("\n".join(seen) + "\n")
    return seen.index(sha) + 1

def comment(login, verdict, sha, items):
    marker = json.dumps({"v": 1, "verdict": verdict, "sha": sha, "summary": "looked at it", "items": items})
    return {"user": {"login": login}, "body": f"<!-- factory:review {marker} -->\nVerdict: {verdict}"}

def match(pattern):
    return re.fullmatch(pattern, path)

if method == "GET" and path.startswith("repos/o/r/issues?"):
    # GitHub does not answer while this file is there.
    if os.path.exists(f"{stub}/gh.down"):
        sys.stderr.write("error connecting to api.github.com\n")
        sys.exit(1)
    label = re.search(r"labels=([^&]*)", path).group(1)
    listed = [issue(n) for n in issues()]
    out([i for i in listed if i["state"] == "open" and any(l["name"] == label for l in i["labels"])])
elif method == "GET" and (found := match(r"repos/o/r/issues/(\d+)")):
    out(issue(found.group(1)))
elif method == "GET" and (found := match(r"repos/o/r/issues/(\d+)/dependencies/blocked_by.*")):
    out([{"number": n, "state": issue(n)["state"]} for n in issues()[found.group(1)].get("blocked_by", [])])
elif method == "POST" and path == "repos/o/r/pulls":
    head = re.search(r"factory/issue-(\d+)-", json.loads(body)["head"])
    number = int(head.group(1)) if head else 90  # 90 stands for a plan pull request
    save("pr", number, body)
    out({"number": number + 2, "html_url": f"https://example.invalid/o/r/pull/{number + 2}"})
elif method == "PATCH" and (found := match(r"repos/o/r/pulls/(\d+)")):
    if json.loads(body).get("state") == "closed":
        open(f"{stub}/pr-closed-{int(found.group(1)) - 2}", "w").write("closed")
    else:
        save("pr.update", int(found.group(1)) - 2, body)
    out({})
elif method == "GET" and path.startswith("repos/o/r/pulls?"):
    # The open pull request for a branch, while its branch exists and it was not closed.
    branch = re.search(r"head=o:([^&]*)", path).group(1).replace("%2F", "/")
    issue_number = re.search(r"issue-(\d+)-", branch)
    number = int(issue_number.group(1)) if issue_number else 90
    is_open = os.path.exists(f"{stub}/pr-{number}.json") and not os.path.exists(f"{stub}/pr-closed-{number}") and git("rev-parse", "-q", "--verify", f"refs/heads/{branch}")
    out([{"number": number + 2, "html_url": f"https://example.invalid/o/r/pull/{number + 2}"}] if is_open else [])
elif method == "GET" and (found := match(r"repos/o/r/pulls/(\d+)")):
    out({"merged": os.path.exists(f"{stub}/merge-{int(found.group(1)) - 2}.json")})
elif method == "GET" and "/check-runs" in path:
    n = nth(path.split("/commits/")[1].split("/")[0])
    runs = []
    def run(number, name, conclusion):
        runs.append({"id": number, "name": name, "status": "completed", "conclusion": conclusion, "html_url": f"https://example.invalid/runs/{number}"})
    run(11, "review", "failure" if mode == "changes-once" and n == 1 else "success")
    if mode == "red-once":
        run(12, "build", "failure" if n == 1 else "success")
    if mode == "always-red":
        run(12, "build", "failure")
    out({"total_count": len(runs), "check_runs": runs})
elif method == "GET" and (found := match(r"repos/o/r/issues/(\d+)/comments.*")):
    number = int(found.group(1)) - 2
    sha = git("rev-parse", branch_of(number))
    n = nth(sha)
    bot = "github-actions[bot]"
    if mode == "no-verdict":
        out([])
    elif mode == "stale-verdict":
        out([comment(bot, "approve", "0" * 40, [])])
    elif mode == "changes-once" and n == 1:
        # Anyone can type the marker. Only the workflow's own comment counts.
        out([comment(bot, "changes", sha, ["Sources/A.swift: say why"]), comment("mallory", "approve", sha, [])])
    else:
        out([comment(bot, "changes", "0" * 40, ["for another commit"]), comment(bot, "approve", sha, [])])
elif method == "GET" and "/actions/jobs/" in path:
    print("step one ok\nerror: the build broke on line 3")
elif method == "PUT" and (found := match(r"repos/o/r/pulls/(\d+)/merge")):
    number = int(found.group(1)) - 2
    if os.environ.get("STUB_REAL_MERGE"):
        # Squash the branch into dev, as GitHub would, so later issues start from it.
        # GitHub takes merges one at a time, and takes one whose branch is behind.
        import fcntl
        turn = open(f"{stub}/merge.lock", "w")
        fcntl.flock(turn, fcntl.LOCK_EX)
        branch = branch_of(number)
        behind = subprocess.run(["git", "--git-dir", origin, "merge-base", "--is-ancestor", "dev", branch]).returncode != 0
        # Behind means: what was verified is not what dev will hold after this merge.
        open(f"{stub}/events", "a").write(f"merge {number} {'behind' if behind else 'current'}\n")
        work = f"{stub}/merge-work-{number}"
        run = lambda *c: subprocess.run(["git", *c], cwd=work, check=True, capture_output=True)
        subprocess.run(["git", "clone", "--quiet", origin, work], check=True)
        run("merge", "--squash", f"origin/{branch}")
        run("commit", "--quiet", "-m", json.loads(body)["commit_title"])
        run("push", "--quiet", "origin", "dev")
    save("merge", number, body)
    open(f"{stub}/closed-{number}", "w").write("merged")
    out({"merged": True})
elif method == "DELETE" and path.startswith("repos/o/r/git/refs/heads/"):
    open(f"{stub}/deleted", "w").write(path)
    if os.environ.get("STUB_REAL_DELETE"):
        git("update-ref", "-d", path.replace("repos/o/r/git/", ""))
elif method == "GET":
    out([])
else:
    out({})
"#;

const CODEX: &str = r#"#!/bin/bash
# Stands in for the developer. What it writes depends on STUB_DEV.
# Two calls may start in the same instant, so the count is taken under a lock.
until mkdir "$STUB_DIR/codex.lock" 2>/dev/null; do sleep 0.01; done
n=$(( $(cat "$STUB_DIR/codex.count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$STUB_DIR/codex.count"
rmdir "$STUB_DIR/codex.lock"
echo "$*" > "$STUB_DIR/codex.args.$n"
cat > "$STUB_DIR/codex.prompt.$n"
out=""; model=""; prev=""
for arg in "$@"; do [ "$prev" = "-o" ] && out="$arg"; [ "$prev" = "-m" ] && model="$arg"; prev="$arg"; done
# Which login and model the engine chose for this call.
echo "codex ${STUB_WHO:-default} ${model:-none}" >> "$STUB_DIR/who"
# What Codex keeps for each session: how much of the plan is used after the reply.
session_file() {
  [ -n "$STUB_CODEX_USED" ] || return 0
  mkdir -p "$CODEX_HOME/sessions/2026/11/24"
  echo "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"primary\":{\"used_percent\":$STUB_CODEX_USED.0,\"window_minutes\":10080,\"resets_at\":4102444800},\"secondary\":null,\"plan_type\":\"stub\"}}}" >> "$CODEX_HOME/sessions/2026/11/24/rollout-2026-11-24T00-00-00-$1.jsonl"
}
if [ "$STUB_CODEX_LIMIT" = "always" ]; then
  echo '{"type":"error","message":"You have hit your usage limit. Try again at 3:10 PM."}'
  exit 1
fi
if grep -q "^You are the reviewer" "$STUB_DIR/codex.prompt.$n"; then
  # The vendors were swapped: Codex reviews what Claude developed.
  echo review >> "$STUB_DIR/codex.roles"
  sha=$(sed -n 's/.*Commit under review: `\([0-9a-f]*\)`.*/\1/p' "$STUB_DIR/codex.prompt.$n" | head -1)
  echo '{"thread_id":"stub-review-session"}'
  session_file stub-review-session
  echo "{\"verdict\":\"approve\",\"head_sha\":\"$sha\",\"summary\":\"looked at it\",\"items\":[]}" > "$out"
  exit 0
fi
echo develop >> "$STUB_DIR/codex.roles"
mkdir -p Sources
if [ "$STUB_DEV" = "by-issue" ]; then
  # Each issue gets its own folder. The pause makes overlap in time visible.
  issue=$(sed -n 's/^# Issue #\([0-9]*\):.*/\1/p' "$STUB_DIR/codex.prompt.$n" | head -1)
  if [ -n "$issue" ] && [ "$STUB_CODEX_LIMIT_ISSUE" = "$issue" ] && [ ! -f "$STUB_DIR/codex.limited" ]; then
    # The usage limit is reached once, on this issue's first call, and only
    # once another issue's developer is at work: that call must go through.
    for _ in $(seq 1 100); do grep -q "^dev-start" "$STUB_DIR/events" 2>/dev/null && break; sleep 0.1; done
    touch "$STUB_DIR/codex.limited"
    echo '{"type":"error","message":"You have hit your usage limit. Try again at 3:10 PM."}'
    exit 1
  fi
  echo "dev-start $issue" >> "$STUB_DIR/events"
  sleep 1
  mkdir -p "Sources/I$issue"
  printf 'let issue = %s\n// FIXED\n' "$issue" > "Sources/I$issue/File.swift"
  echo "dev-end $issue" >> "$STUB_DIR/events"
fi
if [ "$STUB_DEV" = "limit-once" ] && [ "$n" = 1 ]; then
  # What Codex does when the account's usage limit is reached.
  echo '{"type":"error","message":"You have hit your usage limit. Try again at 3:10 PM."}'
  exit 1
fi
case "$STUB_DEV" in
  limit-once)
    printf 'let a = 1\n// FIXED\n' > Sources/A.swift ;;
  fix-on-second)
    if [ "$n" = 1 ]; then echo "let a = 1" > Sources/A.swift; else echo "// FIXED" >> Sources/A.swift; fi ;;
  right-first-time)
    if [ "$n" = 1 ]; then printf 'let a = 1\n// FIXED\n' > Sources/A.swift; else echo "let renamed = 1" >> Sources/A.swift; fi ;;
  never)
    echo "// attempt $n" >> Sources/A.swift ;;
  never-breaks)
    printf 'let a = %s\n// FIXED\n' "$n" > Sources/A.swift ;;
  outside-scope-first)
    printf 'let a = 1\n// FIXED\n' > Sources/A.swift
    if [ "$n" = 1 ]; then mkdir -p Other; echo x > Other/X.swift; else rm -rf Other; fi ;;
  base-moves)
    # Someone else merges into dev while the developer works.
    if [ "$n" = 1 ]; then
      printf 'let a = 1\n// FIXED\n' > Sources/A.swift
      git clone --quiet "$STUB_DIR/../origin.git" "$STUB_DIR/elsewhere"
      ( cd "$STUB_DIR/elsewhere" && echo "from another issue" > Other.txt && git add -A && git commit --quiet -m "feat: another issue" && git push --quiet origin dev )
    fi ;;
  asks-for-a-flow-change)
    if [ "$n" = 1 ]; then
      printf 'let id = "a.b"\n// FIXED\n' > Sources/A.swift
      request='"a.yaml: drop the last assertion, the criteria do not ask for it"'
    else
      echo "// meets the flow as written" >> Sources/A.swift
    fi ;;
esac
echo '{"thread_id":"stub-session"}'
session_file stub-session
echo "{\"summary\":\"attempt $n\",\"commit_subject\":\"feat(a): attempt $n\",\"files_changed\":[\"Sources/A.swift\"],\"verify_ran\":\"not_run\",\"notes\":\"\",\"flow_change_request\":${request:-null}}" > "$out"
"#;

const CLAUDE: &str = r#"#!/bin/bash
# Stands in for Claude in whichever role the prompt gives it: the acceptance
# writer, the same writer ruling on a flow change request, or the reviewer.
until mkdir "$STUB_DIR/claude.lock" 2>/dev/null; do sleep 0.01; done
n=$(( $(cat "$STUB_DIR/claude.count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$STUB_DIR/claude.count"
rmdir "$STUB_DIR/claude.lock"
echo "$*" > "$STUB_DIR/claude.args.$n"
cat > "$STUB_DIR/claude.prompt.$n"
reply() { echo "{\"type\":\"result\",\"is_error\":false,\"session_id\":\"stub-claude-$n\",\"structured_output\":$1}"; }
# Which login and model the engine chose for this call. A provider's env
# file names the login.
model=""; prev=""
for arg in "$@"; do [ "$prev" = "--model" ] && model="$arg"; prev="$arg"; done
echo "claude ${STUB_WHO:-default} ${model:-none}" >> "$STUB_DIR/who"
# What Claude prints with each reply in its stream: how much of the plan is used.
if [ -n "$STUB_CLAUDE_USED" ]; then
  echo "{\"type\":\"system\",\"subtype\":\"init\"}"
  echo "{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"allowed\",\"rateLimitType\":\"five_hour\",\"unifiedWindows\":{\"five_hour\":{\"utilization\":$STUB_CLAUDE_USED,\"resetsAt\":4102444800,\"status\":\"allowed\"}}}}"
fi
if [ -n "$STUB_CLAUDE_LIMIT" ] && [ "$STUB_CLAUDE_LIMIT" = "${STUB_WHO:-default}" ]; then
  echo '{"type":"result","is_error":true,"result":"API Error: Rate limit reached"}'
  exit 1
fi
if grep -q "^You decide what happens next" "$STUB_DIR/claude.prompt.$n"; then
  echo judgment >> "$STUB_DIR/claude.roles"
  j=$(( $(cat "$STUB_DIR/judge.count" 2>/dev/null || echo 0) + 1 ))
  echo "$j" > "$STUB_DIR/judge.count"
  cp "$STUB_DIR/claude.prompt.$n" "$STUB_DIR/judge.prompt.$j"
  part() { echo "{\"key\":\"$1\",\"title\":\"Part $1\",\"summary\":\"One half.\",\"scope\":[\"$2\"],\"accept\":[\"It works\"],\"flows\":[],\"accessibility_ids\":[],\"depends_on\":[]}"; }
  good="[$(part part-one Sources/P1/),$(part part-two Sources/P2/)]"
  # Two unordered issues on the same path: the plan validator refuses this.
  bad="[$(part part-one Sources/P1/),$(part part-two Sources/P1/)]"
  case "$STUB_JUDGE" in
    retry) reply '{"action":"retry","reason":"The developer went down a wrong path.","split":[]}' ;;
    research) reply '{"action":"research","reason":"Find out which marker verify wants.","split":[]}' ;;
    switch) reply '{"action":"switch-vendor","reason":"The same developer failed the same way.","split":[]}' ;;
    dance) reply '{"action":"merge-anyway","reason":"It is close enough.","split":[]}' ;;
    split) reply "{\"action\":\"split\",\"reason\":\"It mixes two changes.\",\"split\":$good}" ;;
    bad-split-then-good)
      if [ "$j" = 1 ]; then reply "{\"action\":\"split\",\"reason\":\"It mixes two changes.\",\"split\":$bad}"
      else reply "{\"action\":\"split\",\"reason\":\"It mixes two changes.\",\"split\":$good}"; fi ;;
    *) reply '{"action":"escalate","reason":"A person should look.","split":[]}' ;;
  esac
  exit 0
fi
if grep -q "^You are the researcher" "$STUB_DIR/claude.prompt.$n"; then
  echo research >> "$STUB_DIR/claude.roles"
  cp "$STUB_DIR/claude.prompt.$n" "$STUB_DIR/research.prompt"
  issue=$(sed -n 's/^# Issue #\([0-9]*\):.*/\1/p' "$STUB_DIR/claude.prompt.$n" | head -1)
  mkdir -p research
  if [ "$STUB_RESEARCH" = "no-sources" ]; then
    printf '## Findings\n\nTrust me.\n' > "research/issue-$issue.md"
    reply '{"summary":"Trust me.","sources":[],"open_questions":[]}'
  else
    printf '## Findings\n\nUse the store.\n\n## Sources\n\n- verify.sh: what the check looks for\n' > "research/issue-$issue.md"
    reply '{"summary":"Use the store.","sources":["verify.sh: what the check looks for"],"open_questions":["Is it fast enough?"]}'
  fi
  exit 0
fi
if grep -q "^You write the summary" "$STUB_DIR/claude.prompt.$n"; then
  echo summary >> "$STUB_DIR/claude.roles"
  if [ "$STUB_SUMMARY" = "broken" ]; then
    echo '{"type":"result","is_error":true,"result":"overloaded"}'
  else
    reply '{"what_changed":"Adds the marker file.","how_it_went":"A clean pass.","for_a_human":["Look at A.swift once"]}'
  fi
  exit 0
fi
if grep -q "^You are the developer\|^Your work on issue" "$STUB_DIR/claude.prompt.$n"; then
  # The vendors were swapped: Claude develops, and gets it right. A fix
  # round's prompt opens differently, and it may reach a new developer.
  echo develop >> "$STUB_DIR/claude.roles"
  mkdir -p Sources
  printf 'let a = 1\n// FIXED\n' > Sources/A.swift
  reply '{"summary":"claude did it","commit_subject":"feat(a): by the other vendor","files_changed":["Sources/A.swift"],"verify_ran":"not_run","notes":"","flow_change_request":null}'
  exit 0
fi
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
if [ -f "$STUB_DIR/issues.json" ]; then
  # Several issues share this "simulator". Leave a trace of who used it when.
  name=$(basename "$(pwd)")
  echo "verify-start $name slot${FACTORY_SLOT_SIMULATOR:-none}" >> "$STUB_DIR/events"
  # What the scheduler's board says at this moment, one board to a line.
  for board in "$FACTORY_HOME"/schedulers/*.json; do
    if [ -f "$board" ]; then tr -d '\n' < "$board" >> "$STUB_DIR/boards"; echo >> "$STUB_DIR/boards"; fi
  done
  # What the slot's note says while this run holds it.
  if [ -n "$FACTORY_SLOT_SIMULATOR" ]; then
    echo "$name $(cat "$FACTORY_HOME/locks/simulator-$FACTORY_SLOT_SIMULATOR.holder" 2>/dev/null)" >> "$STUB_DIR/holders"
  fi
  sleep 1
  echo "verify-end $name slot${FACTORY_SLOT_SIMULATOR:-none}" >> "$STUB_DIR/events"
fi
if grep -rq FIXED Sources 2>/dev/null; then echo "all good"; exit 0; fi
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

/// One issue of a multi-issue trial: number, key, scope, labels, blocked by.
type IssueSpec<'a> = (u64, &'a str, &'a str, &'a [&'a str], &'a [u64]);

/// What the stand-in GitHub keeps for a repository's issues.
fn issues_json(list: &[IssueSpec]) -> String {
    let mut issues = serde_json::Map::new();
    for (number, key, scope, labels, blocked_by) in list {
        let marker = serde_json::json!({"v": 1, "plan": "trial", "key": key, "scope": [scope], "accept": ["It carries the marker"]});
        let body = format!("<!-- factory:issue {marker} -->\n\nDo the work for {key}.\n");
        issues.insert(number.to_string(), serde_json::json!({"title": format!("Issue {key}"), "body": body, "labels": labels, "blocked_by": blocked_by}));
    }
    serde_json::Value::Object(issues).to_string()
}

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

        trial.checkout("repo", "origin.git", workflow);
        trial
    }

    /// A checkout with a workflow and a verify script, and its remote.
    fn checkout(&self, name: &str, remote: &str, workflow: &str) {
        let repo = self.dir.join(name);
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("factory.yaml"), workflow).unwrap();
        executable(&repo.join("verify.sh"), VERIFY);
        let origin = self.dir.join(remote);
        self.git(&self.dir, &["init", "--quiet", "--bare", "-b", "dev", origin.to_str().unwrap()]);
        self.git(&repo, &["init", "--quiet", "-b", "dev"]);
        self.git(&repo, &["add", "-A"]);
        self.git(&repo, &["commit", "--quiet", "-m", "start"]);
        self.git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        self.git(&repo, &["push", "--quiet", "-u", "origin", "dev"]);
        // Plans are proposed into main.
        self.git(&repo, &["push", "--quiet", "origin", "dev:main"]);
        self.git(&repo, &["remote", "set-head", "origin", "dev"]);
    }

    /// Adds a second project, o/r2, with its own issues, and registers both
    /// on this machine in that order. `second` is extra config for the
    /// second one, such as `max_parallel: 1`.
    fn with_second_project(self, workflow: &str, list: &[IssueSpec], second: &str) -> Trial {
        self.checkout("repo2", "origin2.git", workflow);
        fs::create_dir_all(self.dir.join("stub/r2")).unwrap();
        fs::write(self.dir.join("stub/r2/issues.json"), issues_json(list)).unwrap();
        let entry = |name: &str, more: &str| format!("  - dir: {}\n{more}", self.dir.join(name).display());
        let more = if second.is_empty() { String::new() } else { format!("    {second}\n") };
        fs::write(self.dir.join("home/config.yaml"), format!("projects:\n{}{}", entry("repo", ""), entry("repo2", &more))).unwrap();
        self
    }

    /// Starts `factory schedule` on the registered projects, without waiting for it.
    fn schedule_command(&self, flags: &[&str], env: &[(&str, &str)]) -> Command {
        let mut command = self.command(env!("CARGO_BIN_EXE_factory"));
        command
            .arg("schedule")
            .arg("--no-guard")
            .args(flags)
            .envs([("STUB_DEV", "by-issue"), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", "green"), ("STUB_REAL_MERGE", "1")])
            .envs(env.iter().copied())
            .env("FACTORY_POLL_SECONDS", "0")
            .env("FACTORY_SETTLE_SECONDS", "0");
        command
    }

    /// Runs `factory schedule` to its end.
    fn schedule(&self, flags: &[&str]) -> (Output, String) {
        let output = self.schedule_command(flags, &[]).output().unwrap();
        let mut text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        if !output.status.success() {
            // What each issue's own process said, for the failure message.
            for project in ["o__r", "o__r2"] {
                for entry in fs::read_dir(self.dir.join("home/queue").join(project)).into_iter().flatten().flatten() {
                    text.push_str(&format!("\n== {}\n{}", entry.path().display(), fs::read_to_string(entry.path()).unwrap_or_default()));
                }
            }
        }
        (output, text)
    }

    /// The files on `dev` of a remote.
    fn files_on(&self, remote: &str) -> String {
        self.git(&self.dir.join(remote), &["ls-tree", "-r", "--name-only", "dev"])
    }

    fn command(&self, program: &str) -> Command {
        let path = format!("{}:{}", self.dir.join("bin").display(), std::env::var("PATH").unwrap());
        let mut command = Command::new(program);
        command
            .env("PATH", path)
            .env("FACTORY_HOME", self.dir.join("home"))
            // Never the real one: the engine reads Codex's session files.
            .env("CODEX_HOME", self.dir.join("codex-home"))
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

    /// Runs `factory run 7 --no-pr` with this environment.
    fn run_with(&self, env: &[(&str, &str)]) -> (Output, String) {
        self.factory(&["--no-pr"], env)
    }

    fn run_ruling(&self, dev: &str, review: &str, ruling: &str) -> (Output, String) {
        self.factory(&["--no-pr"], &[("STUB_DEV", dev), ("STUB_REVIEW", review), ("STUB_RULING", ruling)])
    }

    /// Runs `factory run 7` all the way to GitHub, which answers as `github` says.
    fn run_to_github(&self, dev: &str, github: &str, flags: &[&str]) -> (Output, String) {
        self.factory(flags, &[("STUB_DEV", dev), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", github)])
    }

    /// Replaces the single issue with several. Each entry is
    /// `(number, key, scope, labels, blocked by)`.
    fn with_issues(self, list: &[IssueSpec]) -> Trial {
        fs::write(self.dir.join("stub/issues.json"), issues_json(list)).unwrap();
        self
    }

    /// Runs `factory queue` with real merges into the stand-in origin.
    fn queue(&self, flags: &[&str]) -> (Output, String) {
        self.queue_with(flags, &[])
    }

    fn queue_with(&self, flags: &[&str], env: &[(&str, &str)]) -> (Output, String) {
        let output = self
            .command(env!("CARGO_BIN_EXE_factory"))
            .arg("queue")
            .args(flags)
            .arg("--repo-dir")
            .arg(self.dir.join("repo"))
            .envs([("STUB_DEV", "by-issue"), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", "green"), ("STUB_REAL_MERGE", "1")])
            .envs(env.iter().copied())
            .env("FACTORY_POLL_SECONDS", "0")
            .env("FACTORY_SETTLE_SECONDS", "0")
            .output()
            .unwrap();
        let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        // Whatever else a queue scenario is about: nothing may merge on a
        // branch that lacks what merged before it, because that was never verified.
        let behind: Vec<String> = self.events().into_iter().filter(|e| e.starts_with("merge ") && e.ends_with(" behind")).collect();
        assert!(behind.is_empty(), "merged without the latest dev: {behind:?} in {:?}\n{text}", self.events());
        (output, text)
    }

    /// Runs any `factory` command against the trial's home.
    fn cli(&self, args: &[&str], env: &[(&str, &str)]) -> (Output, String) {
        let output = self
            .command(env!("CARGO_BIN_EXE_factory"))
            .args(args)
            .envs(env.iter().copied())
            .env("FACTORY_POLL_SECONDS", "0")
            .env("FACTORY_SETTLE_SECONDS", "0")
            .current_dir(self.dir.join("repo"))
            .output()
            .unwrap();
        let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        (output, text)
    }

    /// Gives this machine three subscriptions and the given role lists:
    /// `chatgpt` (Codex), `claude`, and `glm`, which is the Claude program
    /// with another login. `passed` names who has passed its check.
    fn with_providers(self, roles: &str, passed: &[&str]) -> Trial {
        let env_file = self.dir.join("stub/glm-env.sh");
        fs::write(&env_file, "STUB_WHO=glm\n").unwrap();
        let file = format!(
            "providers:\n  chatgpt: {{ cli: codex, family: openai, tiers: {{ standard: gpt-x }} }}\n  claude: {{ cli: claude, family: anthropic, tiers: {{ high: opus }} }}\n  glm: {{ cli: claude, family: zhipu, env_file: {}, tiers: {{ high: glm-big }} }}\nroles:\n{roles}",
            env_file.display()
        );
        fs::write(self.dir.join("home/providers.yaml"), file).unwrap();
        let model = |provider: &str| match provider {
            "chatgpt" => ("standard", "gpt-x"),
            "claude" => ("high", "opus"),
            _ => ("high", "glm-big"),
        };
        let mut checked = serde_json::Map::new();
        for provider in passed {
            let (tier, model) = model(provider);
            let entry = serde_json::json!({"provider": provider, "tier": tier, "model": model, "qualified": true, "structured": true, "wrote_inside": true, "wrote_outside": false, "read_outside": false, "unreliable": false, "checked_at": 1, "dir": "d"});
            checked.insert(format!("{provider}/{tier}"), entry);
        }
        fs::write(self.dir.join("home/providers-checked.json"), serde_json::Value::Object(checked).to_string()).unwrap();
        self
    }

    /// Who answered each agent call, in order: `codex default gpt-x`,
    /// `claude glm glm-big`, ...
    fn who(&self) -> Vec<String> {
        self.stub("who").lines().map(str::to_string).collect()
    }

    /// Sets this machine's pool sizes, as `factory capacity --write` would.
    fn with_pools(self, pools: &str) -> Trial {
        fs::write(self.dir.join("home/config.yaml"), format!("pools: {pools}\n")).unwrap();
        self
    }

    /// The largest number of verify runs inside at once, and every slot that
    /// was used. Panics when two runs held the same slot at the same time.
    fn verify_overlap(&self) -> (usize, std::collections::BTreeSet<String>) {
        let (mut inside, mut most, mut seen) = (Vec::new(), 0, std::collections::BTreeSet::new());
        for event in self.events() {
            let slot = event.rsplit(' ').next().unwrap().to_string();
            if event.starts_with("verify-start") {
                assert!(!inside.contains(&slot), "two verify runs held {slot} at once: {:?}", self.events());
                seen.insert(slot.clone());
                inside.push(slot);
                most = most.max(inside.len());
            } else if event.starts_with("verify-end") {
                inside.retain(|held| held != &slot);
            }
        }
        (most, seen)
    }

    /// The scheduler's board as each verify found it, oldest first.
    fn boards(&self) -> Vec<serde_json::Value> {
        self.stub("boards").lines().map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("not a board: {e}: {line}"))).collect()
    }

    /// What the stand-ins did, in order: `dev-start 7`, `verify-end issue-8 slot1`, ...
    fn events(&self) -> Vec<String> {
        self.stub("events").lines().map(str::to_string).collect()
    }

    fn position(&self, event: &str) -> usize {
        self.events().iter().position(|e| e == event).unwrap_or_else(|| panic!("no event '{event}' in {:?}", self.events()))
    }

    /// Files on the stand-in origin's dev branch.
    fn dev_files(&self) -> String {
        self.git(&self.dir.join("origin.git"), &["ls-tree", "-r", "--name-only", "dev"])
    }

    fn factory(&self, flags: &[&str], env: &[(&str, &str)]) -> (Output, String) {
        let output = self
            .command(env!("CARGO_BIN_EXE_factory"))
            .args(["run", "7"])
            .args(flags)
            .arg("--repo-dir")
            .arg(self.dir.join("repo"))
            .envs(env.iter().copied())
            .env("FACTORY_POLL_SECONDS", "0")
            .env("FACTORY_SETTLE_SECONDS", "0")
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

    /// Subjects of the commits on the pushed branch, oldest first.
    fn pushed(&self) -> Vec<String> {
        let origin = self.dir.join("origin.git");
        self.git(&origin, &["log", "--reverse", "--format=%s", "dev..factory/issue-7-a"]).lines().map(str::to_string).collect()
    }

    fn pushed_head(&self) -> String {
        self.git(&self.dir.join("origin.git"), &["rev-parse", "factory/issue-7-a"])
    }

    fn json(&self, file: &str) -> serde_json::Value {
        serde_json::from_str(&self.stub(file)).unwrap_or(serde_json::Value::Null)
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
    // The rejected attempt was never committed. The one commit holds the
    // whole change, so it is named after the change, not after the fix.
    assert_eq!(trial.commits(), vec!["feat(a): attempt 1"]);
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

/// A workflow that follows the pull request on GitHub, with `rounds` GitHub
/// fix rounds.
fn github_workflow(rounds: u32) -> String {
    format!(
        "workflow:\n  - develop: {{ module: agent/dev, vendor: codex }}\n  - verify:  {{ module: check/verify }}\n  - review:  {{ module: agent/review, vendor: claude }}\non_fail:\n  verify: develop\n  review: develop\ngithub:\n  review_check: review\nlimits:\n  github_fix_rounds: {rounds}\n"
    )
}

#[test]
fn a_green_pull_request_is_merged_at_the_commit_that_was_judged() {
    let trial = Trial::build("merge", &github_workflow(2), ISSUE_BODY);
    let (output, text) = trial.run_to_github("right-first-time", "green", &[]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("MERGED into dev: https://example.invalid/o/r/pull/9"), "{text}");

    let opened = trial.json("pr.json");
    assert_eq!((opened["base"].as_str(), opened["head"].as_str()), (Some("dev"), Some("factory/issue-7-a")));
    assert_eq!(opened["title"], "feat(a): attempt 1");
    assert!(opened["body"].as_str().unwrap().starts_with("Closes #7"), "{opened}");

    let merge = trial.json("merge.json");
    assert_eq!(merge["sha"].as_str().unwrap(), trial.pushed_head());
    assert_eq!(merge["merge_method"], "squash");
    assert_eq!(merge["commit_title"], "feat(a): attempt 1 (#9)");
    assert_eq!(merge["commit_message"], "Closes #7");

    // The branch and the worktree are gone. The run's files stay.
    assert_eq!(trial.stub("deleted"), "repos/o/r/git/refs/heads/factory/issue-7-a");
    assert!(!trial.dir.join("home/worktrees/o__r/issue-7").exists());
    assert!(trial.log().contains("\"step\":\"merge\"") && trial.log().contains("\"verdict\":\"merged\""));
    // A verdict the workflow posted for another commit did not count against this one.
    assert_eq!(trial.stub("codex.count"), "1");
}

#[test]
fn a_github_review_asking_for_changes_starts_a_github_fix_round() {
    let trial = Trial::build("gh-review", &github_workflow(2), ISSUE_BODY);
    let (output, text) = trial.run_to_github("right-first-time", "changes-once", &[]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("passed after 1 fix round(s)"), "{text}");
    assert_eq!(trial.pushed(), vec!["feat(a): attempt 1", "feat(a): attempt 2"]);

    let prompt = trial.stub("codex.prompt.2");
    assert!(prompt.contains("at the `github` step"), "{prompt}");
    assert!(prompt.contains("This is fix round 1 of 2"), "{prompt}");
    assert!(prompt.contains("R1: Sources/A.swift: say why"), "{prompt}");
    assert!(trial.stub("codex.args.2").starts_with("exec resume stub-session"));

    // The fix was verified and reviewed locally before it was pushed, the
    // pull request's body was refreshed, and only then was it merged. The
    // approval typed by someone who is not the workflow never counted.
    assert_eq!(trial.log().matches("\"step\":\"verify\"").count(), 2);
    assert_eq!(trial.stub("claude.count"), "2");
    assert!(trial.json("pr.update.json")["body"].as_str().unwrap().contains("Fix rounds"));
    assert_eq!(trial.json("merge.json")["sha"].as_str().unwrap(), trial.pushed_head());
    assert_eq!(trial.stub("gh.shas").lines().count(), 2);
}

#[test]
fn a_red_check_goes_back_to_the_developer_with_its_log() {
    let trial = Trial::build("gh-red", &github_workflow(2), ISSUE_BODY);
    let (output, text) = trial.run_to_github("right-first-time", "red-once", &[]);
    assert!(output.status.success(), "{text}");
    let prompt = trial.stub("codex.prompt.2");
    assert!(prompt.contains("The check `build` ended as failure"), "{prompt}");
    assert!(prompt.contains("error: the build broke on line 3"), "{prompt}");
    assert!(text.contains("MERGED into dev"), "{text}");
}

#[test]
fn github_fix_rounds_run_out_and_the_pull_request_stays_open() {
    let trial = Trial::build("gh-limit", &github_workflow(1), ISSUE_BODY);
    let (output, text) = trial.run_to_github("never-breaks", "always-red", &[]);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("1 GitHub fix round was used and 'github' still fails"), "{text}");
    assert!(text.contains("the pull request stays open: https://example.invalid/o/r/pull/9"), "{text}");
    assert_eq!(trial.stub("codex.count"), "2");
    assert_eq!(trial.stub("merge.json"), "");
    assert!(trial.dir.join("home/worktrees/o__r/issue-7").exists());
}

#[test]
fn a_green_check_without_a_verdict_for_this_commit_is_not_an_approval() {
    for mode in ["no-verdict", "stale-verdict"] {
        let trial = Trial::build(mode, &github_workflow(2), ISSUE_BODY);
        let (output, text) = trial.run_to_github("right-first-time", mode, &[]);
        assert_eq!(output.status.code(), Some(1), "{mode}: {text}");
        assert!(text.contains("the 'review' check passed without posting a verdict"), "{mode}: {text}");
        // No code change fixes a missing verdict, so the developer is not called again.
        assert_eq!(trial.stub("codex.count"), "1", "{mode}");
        assert_eq!(trial.stub("merge.json"), "", "{mode}");
    }
}

#[test]
fn a_base_branch_that_moved_is_merged_in_and_everything_is_judged_again() {
    let trial = Trial::build("moved", &github_workflow(2), ISSUE_BODY);
    let (output, text) = trial.run_to_github("base-moves", "green", &[]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("dev moved 1 time(s) during the run and was merged in"), "{text}");
    assert_eq!(trial.pushed(), vec!["feat(a): attempt 1", "merge: dev into factory/issue-7-a"]);

    // Verify and review ran again on the merge commit, without the developer.
    assert_eq!(trial.stub("codex.count"), "1");
    assert_eq!(trial.log().matches("\"step\":\"verify\"").count(), 2);
    assert_eq!(trial.stub("claude.count"), "2");
    // The second review judged the merge commit, and its diff holds only this
    // issue's change, not what came in from dev.
    let review = trial.stub("claude.prompt.2");
    assert!(review.contains(&format!("Commit under review: `{}`", trial.pushed_head())), "{review}");
    assert!(review.contains("Sources/A.swift") && !review.contains("Other.txt"), "{review}");
    assert_eq!(trial.json("merge.json")["sha"].as_str().unwrap(), trial.pushed_head());
}

#[test]
fn no_merge_stops_once_github_is_green() {
    let trial = Trial::build("no-merge", &github_workflow(2), ISSUE_BODY);
    let (output, text) = trial.run_to_github("right-first-time", "green", &["--no-merge"]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("Not merged (--no-merge)"), "{text}");
    assert_eq!(trial.stub("merge.json"), "");
    assert!(trial.log().contains("\"step\":\"github\"") && trial.log().contains("\"verdict\":\"green\""));
}

#[test]
fn without_a_github_block_the_run_ends_with_the_pull_request_open() {
    let trial = Trial::new("open-only", 3);
    let (output, text) = trial.factory(&[], &[("STUB_DEV", "right-first-time"), ("STUB_REVIEW", "approve")]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("PASSED every step. Pull request: https://example.invalid/o/r/pull/9"), "{text}");
    assert!(!trial.stub("gh.calls").contains("check-runs"), "nothing was asked about checks");
    assert_eq!(trial.stub("merge.json"), "");
}

#[test]
fn a_pull_request_is_named_after_the_change_even_when_a_gate_held_the_first_attempt() {
    let trial = Trial::build("title", &github_workflow(2), ISSUE_BODY);
    let (output, text) = trial.run_to_github("outside-scope-first", "green", &[]);
    assert!(output.status.success(), "{text}");
    assert_eq!(trial.json("pr.json")["title"], "feat(a): attempt 1");
    assert_eq!(trial.json("merge.json")["commit_title"], "feat(a): attempt 1 (#9)");
    assert!(trial.json("pr.json")["body"].as_str().unwrap().contains("`develop` failed: scope gate"));
}

#[test]
fn an_interrupted_run_picks_up_at_the_step_it_was_on() {
    let trial = Trial::new("resume", 3);
    let mark = trial.dir.join("stub/crash");
    let cut = [("STUB_DEV", "right-first-time"), ("STUB_REVIEW", "approve"), ("FACTORY_TEST_CRASH_AT", "verify"), ("FACTORY_TEST_CRASH_MARK", mark.to_str().unwrap())];
    let (output, text) = trial.factory(&["--no-pr"], &cut);
    assert_eq!(output.status.code(), Some(86), "the run should have been cut off: {text}");
    assert_eq!(trial.stub("codex.count"), "1");

    let (output, text) = trial.factory(&["--no-pr"], &cut);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("resuming run") && text.contains("at 'verify'"), "{text}");
    // The developer's commit was kept. Only what had not finished ran.
    assert_eq!(trial.stub("codex.count"), "1");
    assert_eq!(trial.commits(), vec!["feat(a): attempt 1"]);
    assert_eq!(trial.log().matches("\"step\":\"verify\"").count(), 1);
    assert_eq!(trial.stub("claude.count"), "1");
}

#[test]
fn a_run_cut_off_while_waiting_on_github_does_not_open_a_second_pull_request() {
    let trial = Trial::build("resume-gh", &github_workflow(2), ISSUE_BODY);
    let mark = trial.dir.join("stub/crash");
    let cut = [("STUB_DEV", "right-first-time"), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", "green"), ("FACTORY_TEST_CRASH_AT", "github"), ("FACTORY_TEST_CRASH_MARK", mark.to_str().unwrap())];
    let (output, text) = trial.factory(&[], &cut);
    assert_eq!(output.status.code(), Some(86), "{text}");
    assert!(trial.stub("pr.json").contains("factory/issue-7-a"), "the pull request was open when the run was cut off");

    // Without resume this would be refused, because the branch is on GitHub.
    let (output, text) = trial.factory(&[], &cut);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("at 'github'"), "{text}");
    assert!(text.contains("MERGED into dev"), "{text}");
    assert_eq!(trial.stub("gh.calls").matches("POST repos/o/r/pulls").count(), 1);
    assert_eq!((trial.stub("codex.count").as_str(), trial.stub("claude.count").as_str()), ("1", "1"));
    assert_eq!(trial.json("merge.json")["sha"].as_str().unwrap(), trial.pushed_head());
}

#[test]
fn restart_starts_over_instead_of_picking_up() {
    let trial = Trial::new("restart", 3);
    let mark = trial.dir.join("stub/crash");
    let cut = [("STUB_DEV", "never-breaks"), ("STUB_REVIEW", "approve"), ("FACTORY_TEST_CRASH_AT", "verify"), ("FACTORY_TEST_CRASH_MARK", mark.to_str().unwrap())];
    assert_eq!(trial.factory(&["--no-pr"], &cut).0.status.code(), Some(86));
    let (output, text) = trial.factory(&["--no-pr", "--restart"], &cut);
    assert!(output.status.success(), "{text}");
    assert!(!text.contains("resuming run"), "{text}");
    assert_eq!(trial.stub("codex.count"), "2");
    assert_eq!(trial.commits().len(), 1);
}

#[test]
fn a_usage_limit_is_waited_out_and_is_not_a_failed_attempt() {
    let trial = Trial::new("limit-wait", 3);
    let env = [("STUB_DEV", "limit-once"), ("STUB_REVIEW", "approve"), ("FACTORY_RATE_WAIT_SECONDS", "0")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("codex hit a usage limit"), "{text}");
    assert!(!text.contains("unusable reply"), "{text}");
    assert_eq!(trial.stub("codex.count"), "2");
    assert!(trial.log().contains("\"verdict\":\"rate-limited\""));
    // The limited call left nothing behind. The work is the second call's.
    assert_eq!(trial.commits(), vec!["feat(a): attempt 2"]);
}

#[test]
fn independent_issues_are_developed_at_once_and_share_the_simulator_in_turn() {
    let trial = Trial::build("q-parallel", &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/I7/", &["today"], &[]), (8, "b", "Sources/I8/", &["today"], &[])]);
    let (output, text) = trial.queue(&["--parallel", "2"]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("Blocked: none") && text.contains("Pull request open: none"), "{text}");
    let files = trial.dev_files();
    assert!(files.contains("Sources/I7/File.swift") && files.contains("Sources/I8/File.swift"), "{files}");

    // Both developers were at work before either had finished.
    let first_end = trial.position("dev-end 7").min(trial.position("dev-end 8"));
    assert!(trial.position("dev-start 7") < first_end && trial.position("dev-start 8") < first_end, "{:?}", trial.events());
    // With no pools set there is one simulator: no run started while another was inside.
    let (most, slots) = trial.verify_overlap();
    assert_eq!(most, 1, "two verify runs overlapped: {:?}", trial.events());
    assert_eq!(slots.into_iter().collect::<Vec<_>>(), ["slot1"]);
    // The issue that merged second took in the first one's merge and was judged again.
    assert!(trial.events().iter().filter(|e| e.starts_with("verify-start")).count() >= 3, "{:?}", trial.events());
}

#[test]
fn a_simulator_pool_of_two_lets_two_verifies_in_and_keeps_the_third_out() {
    let trial = Trial::build("q-pool", &github_workflow(2), ISSUE_BODY).with_pools("{ simulator: 2 }").with_issues(&[
        (7, "a", "Sources/I7/", &["today"], &[]),
        (8, "b", "Sources/I8/", &["today"], &[]),
        (9, "c", "Sources/I9/", &["today"], &[]),
    ]);
    let (output, text) = trial.queue(&["--parallel", "3"]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("Blocked: none") && text.contains("Pull request open: none"), "{text}");
    let files = trial.dev_files();
    assert!(["Sources/I7/File.swift", "Sources/I8/File.swift", "Sources/I9/File.swift"].iter().all(|f| files.contains(f)), "{files}");
    // Two at once, never three, and each run was told a slot of its own.
    let (most, slots) = trial.verify_overlap();
    assert_eq!(most, 2, "{:?}", trial.events());
    assert_eq!(slots.into_iter().collect::<Vec<_>>(), ["slot1", "slot2"]);
    // Each run's own log says which slot it was given.
    let logs: String = fs::read_dir(trial.dir.join("home/queue/o__r")).unwrap().map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap()).collect();
    assert!(logs.contains("simulator 1 of 2") && logs.contains("simulator 2 of 2"), "{logs}");
    // While a verify held a slot, the note next to it named that issue and
    // that step. Nothing is left of the notes afterwards.
    let notes = trial.stub("holders");
    for issue in [7, 8, 9] {
        let line = notes.lines().find(|line| line.starts_with(&format!("issue-{issue} "))).unwrap_or_else(|| panic!("no note for issue {issue}: {notes}"));
        assert!(line.contains(&format!("\"issue\":{issue},")) && line.contains("\"pool\":\"simulator\"") && line.contains("\"repo\":\"o/r\"") && line.contains("\"what\":\"verify\""), "{line}");
    }
    let left: Vec<String> = fs::read_dir(trial.dir.join("home/locks")).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().to_string()).filter(|name| name.ends_with(".holder")).collect();
    assert!(left.is_empty(), "{left:?}");
    // The scheduler's own board is gone once it has ended.
    assert_eq!(fs::read_dir(trial.dir.join("home/schedulers")).unwrap().count(), 0);
}

#[test]
fn the_queue_runs_as_many_issues_as_the_simulator_pool_holds() {
    let trial = Trial::build("q-pool-default", &github_workflow(2), ISSUE_BODY).with_pools("{ simulator: 3 }").with_issues(&[
        (7, "a", "Sources/I7/", &["today"], &[]),
        (8, "b", "Sources/I8/", &["today"], &[]),
        (9, "c", "Sources/I9/", &["today"], &[]),
    ]);
    // No --parallel: the pool decides.
    let (output, text) = trial.queue(&[]);
    assert!(output.status.success(), "{text}");
    // All three were started before any had ended.
    assert!(text.contains("up to 3 at once"), "{text}");
    let first_end = text.find(" ended after ").unwrap();
    assert!(["#7 a started", "#8 b started", "#9 c started"].iter().all(|s| text.find(s).is_some_and(|at| at < first_end)), "{text}");
    // Three slots, and nobody was ever given a slot somebody else held.
    let (most, slots) = trial.verify_overlap();
    assert!(most >= 2 && slots.len() >= 2, "{:?}", trial.events());
}

#[test]
fn a_vendor_pool_of_one_makes_developers_take_turns() {
    let trial = Trial::build("q-vendor-pool", &github_workflow(2), ISSUE_BODY)
        .with_pools("{ codex: 1 }")
        .with_issues(&[(7, "a", "Sources/I7/", &["today"], &[]), (8, "b", "Sources/I8/", &["today"], &[])]);
    let (output, text) = trial.queue(&["--parallel", "2"]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("Merged:") && text.contains("Blocked: none"), "{text}");
    let mut inside = 0;
    for event in trial.events() {
        if event.starts_with("dev-start") {
            inside += 1;
            assert_eq!(inside, 1, "two developers were at work at once: {:?}", trial.events());
        } else if event.starts_with("dev-end") {
            inside -= 1;
        }
    }
}

#[test]
fn an_issue_starts_only_after_the_one_it_is_blocked_by_has_merged() {
    let trial = Trial::build("q-blocked", &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/I7/", &["today"], &[]), (8, "b", "Sources/I8/", &["today"], &[7])]);
    let (output, text) = trial.queue(&["--parallel", "2"]);
    assert!(output.status.success(), "{text}");
    assert!(text.find("#7 a started").unwrap() < text.find("#7 a ended").unwrap(), "{text}");
    assert!(text.find("#7 a ended").unwrap() < text.find("#8 b started").unwrap(), "{text}");
    assert!(trial.position("dev-end 7") < trial.position("dev-start 8"));
    // #8 was cut from a dev that already held #7's work, so nothing had to be merged in.
    assert_eq!(trial.events().iter().filter(|e| e.starts_with("verify-start")).count(), 2, "{:?}", trial.events());
    assert!(text.contains("Merged: #7, #8"), "{text}");
}

#[test]
fn issues_whose_scopes_overlap_run_one_after_the_other() {
    let trial = Trial::build("q-overlap", &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/", &["today"], &[]), (8, "b", "Sources/I8/", &["today"], &[])]);
    let (output, text) = trial.queue(&["--parallel", "2"]);
    assert!(output.status.success(), "{text}");
    assert!(trial.position("dev-end 7") < trial.position("dev-start 8"), "{:?}", trial.events());
    assert!(text.contains("Merged: #7, #8"), "{text}");
}

#[test]
fn priority_and_labels_decide_what_runs_and_in_what_order() {
    let trial = Trial::build("q-order", &github_workflow(2), ISSUE_BODY).with_issues(&[
        (7, "low", "Sources/I7/", &["today", "priority:low"], &[]),
        (8, "high", "Sources/I8/", &["today", "priority:high"], &[]),
        (9, "plain", "Sources/I9/", &["today"], &[]),
        (10, "unpicked", "Sources/I10/", &[], &[]),
        (11, "held", "Sources/I11/", &["today", "blocked"], &[]),
        (12, "waiting", "Sources/I12/", &["today"], &[10]),
    ]);
    let (output, text) = trial.queue(&["--parallel", "1"]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("Merged: #8, #9, #7"), "{text}");
    assert!(trial.position("dev-start 8") < trial.position("dev-start 9") && trial.position("dev-start 9") < trial.position("dev-start 7"));
    // Not labelled, held for a person, and blocked by an issue nobody picked.
    assert!(!text.contains("#10 unpicked"), "{text}");
    assert!(text.contains("#11 held not run: it carries the 'blocked' label"), "{text}");
    assert!(text.contains("#12 waiting not run: it is blocked by #10, still open"), "{text}");
    assert!(!trial.events().iter().any(|e| e.ends_with(" 11") || e.ends_with(" 12") || e.ends_with(" 10")), "{:?}", trial.events());
}

/// A workflow with a judgment module, `rounds` fix rounds and `judgments`
/// new attempts.
fn judged_workflow(rounds: u32, judgments: u32) -> String {
    format!(
        "workflow:\n  - develop: {{ module: agent/dev, vendor: codex }}\n  - verify:  {{ module: check/verify }}\n  - review:  {{ module: agent/review, vendor: claude }}\non_fail:\n  verify: develop\n  review: develop\njudgment: {{ module: judgment/stuck, vendor: claude }}\nlimits:\n  fix_rounds: {rounds}\n  judgments: {judgments}\n"
    )
}

#[test]
fn a_judgment_of_retry_starts_the_issue_over_from_nothing() {
    let trial = Trial::build("j-retry", &judged_workflow(0, 2), ISSUE_BODY);
    let env = [("STUB_DEV", "fix-on-second"), ("STUB_REVIEW", "approve"), ("STUB_JUDGE", "retry")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("judgment: retry. The developer went down a wrong path."), "{text}");
    assert!(text.contains("attempt 2, from nothing"), "{text}");
    // A new session and a new worktree: nothing of the first attempt is left.
    assert!(trial.stub("codex.args.2").starts_with("exec --cd "), "{}", trial.stub("codex.args.2"));
    assert_eq!(trial.commits(), vec!["feat(a): attempt 2"]);
    assert_eq!(trial.stub("claude.roles"), "judgment\nreview");
    // The judgment saw what failed and what it may choose.
    let prompt = trial.stub("judge.prompt.1");
    assert!(prompt.contains("stopped at the `verify` step") && prompt.contains("Sources/A.swift lacks the FIXED marker"), "{prompt}");
    assert!(prompt.contains("Available now: `retry`, `switch-vendor`, `split`, `escalate`."), "{prompt}");
    assert!(trial.log().contains("\"step\":\"judgment\"") && trial.log().contains("\"verdict\":\"retry\""));
}

#[test]
fn a_judgment_of_switch_vendor_swaps_who_develops_and_who_reviews() {
    let trial = Trial::build("j-switch", &judged_workflow(0, 2), ISSUE_BODY);
    let env = [("STUB_DEV", "never"), ("STUB_REVIEW", "approve"), ("STUB_JUDGE", "switch")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("the vendors are swapped for the next attempt"), "{text}");
    assert_eq!(trial.stub("claude.roles"), "judgment\ndevelop");
    assert_eq!(trial.stub("codex.roles"), "develop\nreview");
    assert_eq!(trial.commits(), vec!["feat(a): by the other vendor"]);
    // Each vendor is confined as its new role requires.
    assert!(trial.stub("codex.args.2").contains("--sandbox read-only"), "{}", trial.stub("codex.args.2"));
    let developer = trial.stub("claude.args.2");
    assert!(developer.contains("--permission-mode acceptEdits") && developer.contains("blockWritesOutsideWorkingDirectories"), "{developer}");
}

#[test]
fn an_answer_off_the_menu_goes_to_a_person() {
    let trial = Trial::build("j-menu", &judged_workflow(0, 2), ISSUE_BODY);
    let env = [("STUB_DEV", "never"), ("STUB_REVIEW", "approve"), ("STUB_JUDGE", "dance")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("The judgment chose `merge-anyway`, but that is not on the menu"), "{text}");
    assert_eq!(trial.stub("codex.count"), "1");

    // The person's list says what failed, what the judgment said, and what to do.
    let (_, listed) = trial.cli(&["blocked"], &[]);
    assert!(listed.contains("o/r#7 a"), "{listed}");
    assert!(listed.contains("what failed: 0 fix rounds were used and 'verify' still fails"), "{listed}");
    assert!(listed.contains("judgment: escalate. The judgment chose `merge-anyway`"), "{listed}");
    assert!(listed.contains("`factory retry 7` starts it over"), "{listed}");
}

#[test]
fn a_judgment_cannot_keep_an_issue_going_past_its_attempts() {
    let trial = Trial::build("j-limit", &judged_workflow(0, 1), ISSUE_BODY);
    let env = [("STUB_DEV", "never"), ("STUB_REVIEW", "approve"), ("STUB_JUDGE", "retry")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("FAILED at verify, in attempt 2"), "{text}");
    assert!(text.contains("The judgment chose `retry`, but this issue has no new attempts left"), "{text}");
    assert_eq!((trial.stub("codex.count").as_str(), trial.stub("judge.count").as_str()), ("2", "2"));
    // The second judgment was told what was no longer available, and what came before.
    let second = trial.stub("judge.prompt.2");
    assert!(second.contains("Available now: `split`, `escalate`. Not available: `retry`"), "{second}");
    assert!(second.contains("Attempt 1 stopped at `verify`"), "{second}");
}

#[test]
fn a_split_is_proposed_as_a_plan_for_a_person_and_creates_nothing() {
    let trial = Trial::build("j-split", &judged_workflow(0, 2), ISSUE_BODY);
    let env = [("STUB_DEV", "never"), ("STUB_REVIEW", "approve"), ("STUB_JUDGE", "bad-split-then-good")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert_eq!(output.status.code(), Some(1), "the issue waits for a person: {text}");
    // The first proposal broke a plan rule, so the validator's errors went back once.
    assert!(text.contains("the proposed split is not a valid plan, asking once more"), "{text}");
    assert!(trial.stub("judge.prompt.2").contains("overlap on Sources/P1/"), "{}", trial.stub("judge.prompt.2"));
    assert!(text.contains("The split is proposed as a plan in https://example.invalid/o/r/pull/92"), "{text}");

    // A pull request into main, where plans are reviewed, holding one plan file.
    let opened = trial.json("pr.json");
    assert_eq!((opened["base"].as_str(), opened["head"].as_str()), (Some("main"), Some("plan/trial-split-7")));
    let origin = trial.dir.join("origin.git");
    assert_eq!(trial.git(&origin, &["diff", "--name-only", "main", "plan/trial-split-7"]), "plans/trial-split-7.yaml");
    let plan = trial.git(&origin, &["show", "plan/trial-split-7:plans/trial-split-7.yaml"]);
    assert!(plan.contains("plan: trial-split-7") && plan.contains("key: part-one") && plan.contains("key: part-two"), "{plan}");
    // No issue was created, and the developer was not called again.
    assert!(!trial.stub("gh.calls").contains("POST repos/o/r/issues\n"), "{}", trial.stub("gh.calls"));
    assert_eq!(trial.stub("codex.count"), "1");
}

#[test]
fn retry_takes_over_what_a_blocked_run_left_on_github() {
    let trial = Trial::build("j-retry-cli", &github_workflow(0), ISSUE_BODY);
    let stuck = [("STUB_DEV", "never-breaks"), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", "always-red"), ("STUB_REAL_DELETE", "1")];
    let (output, text) = trial.factory(&[], &stuck);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("the pull request stays open"), "{text}");
    let first_head = trial.pushed_head();

    // A plain run refuses, and says how to take over.
    let (output, text) = trial.factory(&[], &stuck);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(text.contains("Use `factory retry 7`"), "{text}");

    // Retrying closes that pull request, deletes that branch, and starts from
    // nothing. With GitHub green this time, it merges.
    let green = [("STUB_DEV", "never-breaks"), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", "green")];
    let (output, text) = trial.cli(&["retry", "7"], &[green[0], green[1], green[2], ("STUB_REAL_DELETE", "1")]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("closing https://example.invalid/o/r/pull/9, which an earlier attempt opened"), "{text}");
    assert!(trial.dir.join("stub/pr-closed-7").exists());
    assert!(text.contains("MERGED into dev"), "{text}");
    assert_ne!(trial.json("merge.json")["sha"].as_str().unwrap(), first_head, "the merged commit is the new attempt's");
    assert_eq!(trial.stub("gh.calls").matches("POST repos/o/r/pulls\n").count(), 2);
}

#[test]
fn drop_takes_an_issue_off_the_queue_and_removes_what_the_engine_made() {
    let trial = Trial::build("j-drop", &github_workflow(0), ISSUE_BODY);
    let stuck = [("STUB_DEV", "never-breaks"), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", "always-red"), ("STUB_REAL_DELETE", "1")];
    assert_eq!(trial.factory(&[], &stuck).0.status.code(), Some(1));
    assert!(trial.cli(&["blocked"], &[]).1.contains("pull request: https://example.invalid/o/r/pull/9"));

    let (output, text) = trial.cli(&["drop", "7"], &stuck);
    assert!(output.status.success(), "{text}");
    assert!(trial.dir.join("stub/pr-closed-7").exists(), "{text}");
    assert!(!trial.dir.join("home/worktrees/o__r/issue-7").exists());
    assert!(trial.git(&trial.dir.join("origin.git"), &["for-each-ref", "refs/heads/factory"]).is_empty());
    let calls = trial.stub("gh.calls");
    for label in ["today", "in-progress", "blocked"] {
        assert!(calls.contains(&format!("DELETE repos/o/r/issues/7/labels/{label}")), "{calls}");
    }
    // It no longer waits for a person, and its run files are still there.
    assert!(trial.cli(&["blocked"], &[]).1.contains("no issue is waiting for a person"));
    assert!(trial.dir.join("home/runs/o__r/issue-7").exists());
}

/// A workflow with a research step, a summary and the GitHub loop.
fn full_workflow() -> String {
    "workflow:\n  - research: { module: agent/research, vendor: claude }\n  - develop:  { module: agent/dev, vendor: codex }\n  - verify:   { module: check/verify }\n  - review:   { module: agent/review, vendor: claude }\non_fail:\n  verify: develop\n  review: develop\ngithub:\n  review_check: review\nsummary: { module: agent/summary, vendor: claude }\njudgment: { module: judgment/stuck, vendor: claude }\nlimits:\n  fix_rounds: 0\n".to_string()
}

fn labelled(trial: &Trial, labels: &[&str]) {
    let issue = serde_json::json!({ "number": 7, "state": "open", "title": "Add the file", "body": ISSUE_BODY, "labels": labels });
    fs::write(trial.dir.join("stub/issue.json"), issue.to_string()).unwrap();
}

#[test]
fn an_issue_that_asks_for_research_gets_notes_before_anyone_develops() {
    let trial = Trial::build("research", &full_workflow(), ISSUE_BODY);
    labelled(&trial, &["needs-research"]);
    let (output, text) = trial.run_to_github("right-first-time", "green", &[]);
    assert!(output.status.success(), "{text}");
    assert_eq!(trial.pushed(), vec!["docs(research): notes for #7", "feat(a): attempt 1"]);
    assert_eq!(trial.stub("claude.roles"), "research\nreview\nsummary");
    // The researcher may use the web and writes one file. The developer is pointed at it.
    assert!(trial.stub("claude.args.1").contains("WebSearch WebFetch"), "{}", trial.stub("claude.args.1"));
    assert!(trial.stub("research.prompt").contains("The issue is labelled `needs-research`"));
    let developer = trial.stub("codex.prompt.1");
    assert!(developer.contains("Read `research/issue-7.md` before you start. Its main finding: Use the store."), "{developer}");

    // The pull request leads with the summary written for a person, and
    // keeps the facts the engine recorded.
    let body = trial.json("pr.json")["body"].as_str().unwrap().to_string();
    assert!(body.starts_with("Closes #7\n\n## Summary\n\nAdds the marker file.\n\nA clean pass."), "{body}");
    assert!(body.contains("## Research") && body.contains("`research/issue-7.md`"), "{body}");
    assert!(body.contains("## Verification") && body.contains("Scope gate: passed"), "{body}");
    assert!(body.contains("- Look at A.swift once") && body.contains("- Open question from research: Is it fast enough?"), "{body}");
    assert!(text.contains("MERGED into dev"), "{text}");

    // The day's handoff repeats it, from what was recorded.
    let out = trial.dir.join("handoffs");
    let (output, listed) = trial.cli(&["handoff", "--out", out.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{listed}");
    let file = fs::read_dir(out.join("r")).unwrap().next().unwrap().unwrap().path();
    let handoff = fs::read_to_string(&file).unwrap();
    assert!(handoff.contains("| #7 Add the file | Merged | https://example.invalid/o/r/pull/9 |"), "{handoff}");
    assert!(handoff.contains("- Look at A.swift once") && handoff.contains("Adds the marker file."), "{handoff}");
    assert!(handoff.contains("## Needs a person\n\nNothing."), "{handoff}");
}

#[test]
fn research_is_skipped_unless_asked_for_and_must_name_its_sources() {
    let trial = Trial::build("research-skip", &full_workflow(), ISSUE_BODY);
    let (output, text) = trial.run_to_github("right-first-time", "green", &[]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("research: skipped, the issue does not ask for research"), "{text}");
    assert_eq!(trial.stub("claude.roles"), "review\nsummary");
    assert!(trial.stub("codex.prompt.1").contains("None was needed for this issue."));

    let trial = Trial::build("research-bare", &full_workflow(), ISSUE_BODY);
    labelled(&trial, &["needs-research"]);
    let (output, text) = trial.factory(&[], &[("STUB_DEV", "right-first-time"), ("STUB_REVIEW", "approve"), ("STUB_RESEARCH", "no-sources"), ("STUB_JUDGE", "escalate")]);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("must list its sources under `## Sources`"), "{text}");
    assert_eq!(trial.stub("codex.count"), "", "nobody develops from notes without sources");
}

#[test]
fn a_judgment_can_send_an_issue_to_research_first() {
    let trial = Trial::build("research-judged", &full_workflow(), ISSUE_BODY);
    let env = [("STUB_DEV", "fix-on-second"), ("STUB_REVIEW", "approve"), ("STUB_JUDGE", "research"), ("STUB_GITHUB", "green")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("judgment: research. Find out which marker verify wants."), "{text}");
    // The first attempt had no research. The second starts with it, on the judgment's question.
    assert_eq!(trial.stub("claude.roles"), "judgment\nresearch\nreview");
    assert!(trial.stub("judge.prompt.1").contains("`research`, `split`, `escalate`"), "{}", trial.stub("judge.prompt.1"));
    assert!(trial.stub("research.prompt").contains("Find out which marker verify wants."));
    assert_eq!(trial.commits(), vec!["docs(research): notes for #7", "feat(a): attempt 2"]);
}

#[test]
fn a_failed_summary_never_holds_up_a_pull_request() {
    let trial = Trial::build("summary-broken", &full_workflow(), ISSUE_BODY);
    let env = [("STUB_DEV", "right-first-time"), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", "green"), ("STUB_SUMMARY", "broken")];
    let (output, text) = trial.factory(&[], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("summary: none"), "{text}");
    let body = trial.json("pr.json")["body"].as_str().unwrap().to_string();
    assert!(body.starts_with("Closes #7\n\n## What changed\n\nattempt 1"), "{body}");
    assert!(text.contains("MERGED into dev"), "{text}");
}

#[test]
fn promote_opens_one_pull_request_into_main_and_never_merges_it() {
    let trial = Trial::build("promote", &github_workflow(2), ISSUE_BODY);
    let (output, text) = trial.cli(&["promote"], &[]);
    assert!(output.status.success() && text.contains("nothing to promote"), "{text}");

    let merged = [("STUB_DEV", "right-first-time"), ("STUB_REVIEW", "approve"), ("STUB_GITHUB", "green"), ("STUB_REAL_MERGE", "1")];
    assert!(trial.factory(&[], &merged).0.status.success());
    let (output, text) = trial.cli(&["promote"], &[]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("opened https://example.invalid/o/r/pull/92: 1 change(s). A person approves and merges it"), "{text}");
    let opened = trial.json("pr-90.json");
    assert_eq!((opened["base"].as_str(), opened["head"].as_str()), (Some("main"), Some("dev")));
    assert_eq!(opened["title"], "promote: dev to main (1 change)");
    assert!(opened["body"].as_str().unwrap().contains("- feat(a): attempt 1 (#9)"), "{opened}");

    // Asked again, it refreshes that pull request and opens no second one.
    let (_, text) = trial.cli(&["promote"], &[]);
    assert!(text.contains("refreshed https://example.invalid/o/r/pull/92"), "{text}");
    let calls = trial.stub("gh.calls");
    assert_eq!(calls.matches("POST repos/o/r/pulls\n").count(), 2, "one for the issue, one for the promotion: {calls}");
    assert!(!calls.contains("PUT repos/o/r/pulls/92/merge"), "{calls}");
}

#[test]
fn a_workflow_change_is_checked_and_then_proposed_as_a_pull_request() {
    let trial = Trial::build("wf-propose", &github_workflow(2), ISSUE_BODY);
    let (output, text) = trial.cli(&["workflow", "validate"], &[]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("workflow OK: develop -> verify -> review"), "{text}");
    assert!(text.contains("a failed verify goes back to develop"), "{text}");

    // A draft that breaks the rules is never proposed.
    let bad = trial.dir.join("bad.yaml");
    fs::write(&bad, github_workflow(2).replace("  verify: develop\n", "").replace("vendor: claude", "vendor: codex")).unwrap();
    let (output, text) = trial.cli(&["workflow", "propose", "--file", bad.to_str().unwrap()], &[]);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("step 'verify' can fail and has no on_fail"), "{text}");
    assert!(text.contains("step 'review' must use a different vendor"), "{text}");
    assert!(!trial.stub("gh.calls").contains("POST repos/o/r/pulls"), "{}", trial.stub("gh.calls"));

    // A valid one becomes a pull request into the default branch, and dev itself is untouched.
    let good = trial.dir.join("good.yaml");
    fs::write(&good, github_workflow(1)).unwrap();
    let (output, text) = trial.cli(&["workflow", "propose", "--file", good.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("opened https://example.invalid/o/r/pull/92. A person merges it"), "{text}");
    let opened = trial.json("pr-90.json");
    assert_eq!(opened["base"], "dev");
    let branch = opened["head"].as_str().unwrap().to_string();
    assert!(branch.starts_with("factory/workflow-"), "{branch}");
    let origin = trial.dir.join("origin.git");
    assert_eq!(trial.git(&origin, &["diff", "--name-only", "dev", &branch]), "factory.yaml");
    assert!(trial.git(&origin, &["show", &format!("{branch}:factory.yaml")]).contains("github_fix_rounds: 1"));
    assert!(trial.git(&origin, &["show", "dev:factory.yaml"]).contains("github_fix_rounds: 2"));
    assert!(!trial.stub("gh.calls").contains("/merge"), "{}", trial.stub("gh.calls"));

    // Proposing what is already there changes nothing.
    fs::write(&good, github_workflow(2)).unwrap();
    let (output, text) = trial.cli(&["workflow", "propose", "--file", good.to_str().unwrap()], &[]);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(text.contains("nothing would change"), "{text}");
}

#[test]
fn a_new_module_is_scaffolded_into_a_pull_request() {
    let trial = Trial::build("module-new", &github_workflow(2), ISSUE_BODY);
    let (_, listed) = trial.cli(&["module", "list"], &[]);
    assert!(listed.contains("check/verify  check, built-in") && listed.contains("agent/dev  agent (dev), built-in"), "{listed}");
    assert!(!listed.contains("check/lint"), "{listed}");

    let (_, shown) = trial.cli(&["module", "new", "check/lint", "--dry-run"], &[]);
    assert!(shown.contains("--- modules/check/lint/module.yaml") && shown.contains("run: ./modules/check/lint/run.sh"), "{shown}");
    assert!(!trial.stub("gh.calls").contains("POST repos/o/r/pulls"));

    let (output, text) = trial.cli(&["module", "new", "check/lint"], &[]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("opened https://example.invalid/o/r/pull/92 with 2 file(s)"), "{text}");
    let origin = trial.dir.join("origin.git");
    let files = trial.git(&origin, &["ls-tree", "-r", "factory/module-check-lint", "modules/"]);
    // The script is executable, and fails until someone writes it.
    assert!(files.contains("100755") && files.contains("modules/check/lint/run.sh"), "{files}");
    assert!(files.contains("100644") && files.contains("modules/check/lint/module.yaml"), "{files}");
    assert!(trial.git(&origin, &["show", "factory/module-check-lint:modules/check/lint/run.sh"]).contains("exit 1"));
    assert_eq!(trial.json("pr-90.json")["base"], "dev");

    let (output, text) = trial.cli(&["module", "new", "agent/helper"], &[]);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(text.contains("an agent module needs a role"), "{text}");
}

const ALL_PASSED: &[&str] = &["chatgpt", "claude", "glm"];

#[test]
fn a_role_goes_to_the_next_provider_when_the_first_is_out_of_quota() {
    let roles = "  develop: [{ use: chatgpt, tier: standard }, { use: glm, tier: high }]\n  review: [{ use: claude, tier: high }, { use: glm, tier: high }]\n";
    let trial = Trial::new("p-fallback", 1).with_providers(roles, ALL_PASSED);
    // A long wait: the run must not sit it out when someone else can do the work.
    let env = [("STUB_CODEX_LIMIT", "always"), ("STUB_REVIEW", "approve"), ("FACTORY_RATE_WAIT_SECONDS", "600")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("chatgpt standard (gpt-x, openai) hit a usage limit"), "{text}");
    // Codex was asked once. GLM developed, through the Claude program with
    // its own login and model. Claude, another family, reviewed.
    assert_eq!(trial.who(), ["codex default gpt-x", "claude glm glm-big", "claude default opus"]);
    assert_eq!(trial.stub("claude.roles"), "develop\nreview");
    let log = trial.log();
    assert!(log.contains("\"vendor\":\"glm\"") && log.contains("\"model\":\"glm-big\"") && log.contains("\"subscription\":\"glm\""), "{log}");
    assert!(log.contains("\"verdict\":\"rate-limited\""), "{log}");
}

#[test]
fn each_call_leaves_a_reading_of_its_plan_for_the_whole_machine() {
    let roles = "  develop: [{ use: chatgpt, tier: standard }]\n  review: [{ use: claude, tier: high }]\n";
    let trial = Trial::new("p-readings", 1).with_providers(roles, ALL_PASSED);
    let env = [("STUB_DEV", "right-first-time"), ("STUB_REVIEW", "approve"), ("STUB_CODEX_USED", "12"), ("STUB_CLAUDE_USED", "0.4")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert!(output.status.success(), "{text}");
    // Codex said it in its own session file, Claude in its stream. Neither
    // is out, so nobody was set aside.
    let known: serde_json::Value = serde_json::from_str(&fs::read_to_string(trial.dir.join("home/availability.json")).unwrap()).unwrap();
    assert_eq!(known["quota"]["chatgpt"]["windows"], serde_json::json!(["weekly window: 12% used, resets 2100-01-01 00:00 UTC", "plan: stub"]), "{known}");
    assert_eq!(known["quota"]["claude"]["windows"], serde_json::json!(["5-hour window: 40% used, resets 2100-01-01 00:00 UTC"]), "{known}");
    assert!(known["quota"]["chatgpt"]["out_until"].is_null() && known["out"].as_object().unwrap().is_empty(), "{known}");
    assert!(!text.contains("used up"), "{text}");
    // `factory providers` shows the last reading of each.
    let (_, report) = trial.cli(&["providers"], &[]);
    assert!(report.contains("quota read after each call, last read "), "{report}");
    assert!(report.contains("    weekly window: 12% used") && report.contains("    5-hour window: 40% used"), "{report}");
}

#[test]
fn a_plan_reported_as_used_up_sends_the_next_call_to_someone_else() {
    let roles = "  develop: [{ use: chatgpt, tier: standard }, { use: glm, tier: high }]\n  review: [{ use: claude, tier: high }, { use: glm, tier: high }]\n";
    let trial = Trial::new("p-used-up", 2).with_providers(roles, ALL_PASSED);
    // Codex's first reply works, but leaves the code unfixed and says the
    // plan is at 100%. No call failed, and no usage limit was hit.
    let env = [("STUB_DEV", "fix-on-second"), ("STUB_REVIEW", "approve"), ("STUB_CODEX_USED", "100")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("develop: chatgpt reports its plan used up. Set aside until "), "{text}");
    assert!(!text.contains("hit a usage limit"), "{text}");
    // The fix went to the next in the list without Codex being tried again.
    assert_eq!(trial.who(), ["codex default gpt-x", "claude glm glm-big", "claude default opus"]);
    assert_eq!(trial.stub("codex.count"), "1");
    // The reading holds for an hour at most, though the reset is far away.
    let known: serde_json::Value = serde_json::from_str(&fs::read_to_string(trial.dir.join("home/availability.json")).unwrap()).unwrap();
    let (at, until) = (known["quota"]["chatgpt"]["at"].as_u64().unwrap(), known["quota"]["chatgpt"]["out_until"].as_u64().unwrap());
    assert_eq!(until - at, 3600, "{known}");
}

#[test]
fn the_reviewer_never_comes_from_the_developers_family() {
    // Claude is first in both lists. It develops, so it may not review.
    let roles = "  develop: [{ use: claude, tier: high }]\n  review: [{ use: claude, tier: high }, { use: glm, tier: high }]\n";
    let trial = Trial::new("p-independent", 1).with_providers(roles, ALL_PASSED);
    let (output, text) = trial.factory(&["--no-pr"], &[("STUB_REVIEW", "approve")]);
    assert!(output.status.success(), "{text}");
    assert_eq!(trial.who(), ["claude default opus", "claude glm glm-big"]);
    assert!(text.contains("review: approve by glm"), "{text}");
    // Codex, the workflow file's developer, was never called: the machine's list decides.
    assert_eq!(trial.stub("codex.count"), "");
}

#[test]
fn only_a_provider_that_passed_its_check_is_used() {
    let roles = "  develop: [{ use: chatgpt, tier: standard }, { use: glm, tier: high }]\n  review: [{ use: claude, tier: high }]\n";
    let trial = Trial::new("p-qualified", 1).with_providers(roles, &["claude", "glm"]);
    let (output, text) = trial.factory(&["--no-pr"], &[("STUB_REVIEW", "approve")]);
    assert!(output.status.success(), "{text}");
    assert_eq!(trial.who(), ["claude glm glm-big", "claude default opus"]);

    // With nobody left for a role the issue stops, and says why for each one.
    let roles = "  develop: [{ use: glm, tier: high }]\n  review: [{ use: glm, tier: high }, { use: chatgpt, tier: standard }]\n";
    let trial = Trial::new("p-nobody", 1).with_providers(roles, &["claude", "glm"]);
    let (output, text) = trial.factory(&["--no-pr"], &[("STUB_REVIEW", "approve")]);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("nobody can take the role review"), "{text}");
    assert!(text.contains("glm high (glm-big, zhipu): its family, zhipu, must stay independent here"), "{text}");
    assert!(text.contains("chatgpt standard (gpt-x, openai): has not passed `factory providers check`"), "{text}");
    assert_eq!(trial.who(), ["claude glm glm-big"]);
}

#[test]
fn a_judgment_of_switch_vendor_takes_the_next_family_in_the_list() {
    let roles = "  develop: [{ use: chatgpt, tier: standard }, { use: glm, tier: high }]\n  review: [{ use: claude, tier: high }]\n";
    let trial = Trial::build("p-switch", &judged_workflow(0, 2), ISSUE_BODY).with_providers(roles, ALL_PASSED);
    let env = [("STUB_DEV", "never"), ("STUB_REVIEW", "approve"), ("STUB_JUDGE", "switch")];
    let (output, text) = trial.factory(&["--no-pr"], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("the next attempt's developer comes from another family than openai"), "{text}");
    // Codex failed, the judgment (the workflow's own vendor: no list names
    // that role) chose, GLM developed, Claude reviewed.
    assert_eq!(trial.who(), ["codex default gpt-x", "claude default none", "claude glm glm-big", "claude default opus"]);
}

#[test]
fn a_providers_file_with_a_mistake_stops_the_run_before_any_call() {
    let roles = "  develop: [{ use: nobody, tier: high }]\n";
    let trial = Trial::new("p-mistake", 1).with_providers(roles, ALL_PASSED);
    let (output, text) = trial.factory(&["--no-pr"], &[("STUB_REVIEW", "approve")]);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("providers.yaml has problems") && text.contains("role develop: unknown provider nobody"), "{text}");
    assert!(trial.who().is_empty());
}

#[test]
fn an_issue_parks_when_nobody_has_quota_and_continues_where_it_stopped() {
    let trial = Trial::new("park", 1);
    // The limit lasts 4 seconds. Waits over 1 second park instead of sleeping.
    let env = [("STUB_DEV", "limit-once"), ("STUB_REVIEW", "approve"), ("FACTORY_RATE_WAIT_SECONDS", "4"), ("FACTORY_PARK_AFTER_SECONDS", "1")];
    let (output, text) = trial.run_with(&env);
    assert_eq!(output.status.code(), Some(75), "{text}");
    assert!(text.contains("codex hit a usage limit") && text.contains("PARKED: parked until"), "{text}");
    assert!(text.contains("nobody can take the role develop. codex: out of quota until"), "{text}");
    // It is waiting, not failed, and nobody was asked to look at it.
    let (_, status) = trial.cli(&["status"], &[]);
    assert!(status.contains("waiting") && !status.contains("failed"), "{status}");
    let (_, blocked) = trial.cli(&["blocked"], &[]);
    assert!(blocked.contains("no issue is waiting for a person"), "{blocked}");

    // Picked up too early, it parks again without spending a call.
    let (output, text) = trial.run_with(&env);
    assert_eq!(output.status.code(), Some(75), "{text}");
    assert_eq!(trial.stub("codex.count"), "1");

    // After the wait it continues at the step it stopped at, and passes.
    std::thread::sleep(std::time::Duration::from_secs(5));
    let (output, text) = trial.run_with(&env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("resuming run") && text.contains("at 'develop'"), "{text}");
    assert_eq!(trial.stub("codex.count"), "2");
    assert_eq!(trial.log().matches("\"verdict\":\"rate-limited\"").count(), 1);
    // The good reply cleared the ledger.
    let ledger = fs::read_to_string(trial.dir.join("home/availability.json")).unwrap();
    assert!(!ledger.contains("codex/default"), "{ledger}");
}

#[test]
fn the_queue_goes_on_with_other_work_while_an_issue_is_parked() {
    let trial = Trial::build("q-park", &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/I7/", &["today"], &[]), (8, "b", "Sources/I8/", &["today"], &[])]);
    // Issue 8's developer hits the limit once. Issue 7's call is already under way.
    let env = [("STUB_CODEX_LIMIT_ISSUE", "8"), ("FACTORY_RATE_WAIT_SECONDS", "6"), ("FACTORY_PARK_AFTER_SECONDS", "1")];
    let (output, text) = trial.queue_with(&["--parallel", "2"], &env);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("#8 b parked after"), "{text}");
    assert!(text.contains("queue done. Merged: #7, #8. Pull request open: none. Blocked: none"), "{text}");
    // Issue 7 was merged while 8 waited. 8 then developed, on top of 7's work.
    assert!(text.find("#7 a ended").unwrap() < text.rfind("#8 b ended").unwrap(), "{text}");
    assert!(trial.position("dev-end 7") < trial.position("dev-start 8"), "{:?}", trial.events());
    let files = trial.dev_files();
    assert!(files.contains("Sources/I7/File.swift") && files.contains("Sources/I8/File.swift"), "{files}");
    // 8's second process took up the run the first one left.
    let logs: String = fs::read_dir(trial.dir.join("home/queue/o__r")).unwrap().map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap()).collect();
    assert!(logs.contains("PARKED: parked until") && logs.contains("resuming run"), "{logs}");
}

#[test]
fn the_queue_picks_up_an_issue_that_becomes_ready_while_a_place_is_free() {
    // Issue 8 has no label when the queue starts.
    let trial = Trial::build("q-late", &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/I7/", &["today"], &[]), (8, "b", "Sources/I8/", &[], &[])]);
    let issues = trial.dir.join("stub/issues.json");
    // A person labels it a second later, while issue 7 is still at work.
    let label_later = std::thread::spawn({
        let issues = issues.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let mut all: serde_json::Value = serde_json::from_str(&fs::read_to_string(&issues).unwrap()).unwrap();
            all["8"]["labels"] = serde_json::json!(["today"]);
            fs::write(&issues, all.to_string()).unwrap();
        }
    });
    let (output, text) = trial.queue_with(&["--parallel", "2"], &[("FACTORY_QUEUE_LOOK_SECONDS", "1")]);
    label_later.join().unwrap();
    assert!(output.status.success(), "{text}");
    // It was started without waiting for issue 7 to end.
    assert!(text.find("#8 b started").unwrap() < text.find("#7 a ended").unwrap(), "{text}");
    assert!(text.contains("Merged:") && text.contains("#7") && text.contains("#8") && text.contains("Blocked: none"), "{text}");
}

/// Two projects on one machine: o/r with issues 7 and 8, o/r2 with 17 and 18.
fn two_projects(name: &str, second: &str) -> Trial {
    Trial::build(name, &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/I7/", &["today"], &[]), (8, "b", "Sources/I8/", &["today"], &[])])
        .with_second_project(&github_workflow(2), &[(17, "c", "Sources/I17/", &["today"], &[]), (18, "d", "Sources/I18/", &["today"], &[])], second)
}

#[test]
fn two_projects_share_the_machine_side_by_side() {
    let trial = two_projects("s-side", "");
    let (output, text) = trial.schedule(&["--parallel", "2"]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("schedule: 2 project(s), up to 2 issue(s) at once, projects side by side"), "{text}");
    // The two places went to one issue of each project, not to two of the first.
    let first_end = text.find(" ended after ").unwrap();
    assert!(text.find("r#7 a started").unwrap() < first_end && text.find("r2#17 c started").unwrap() < first_end, "{text}");
    assert!(text.find("r#8 b started").unwrap() > first_end, "{text}");
    // Everything of both was done, each in its own repository.
    for line in ["schedule done for o/r. Merged: ", "schedule done for o/r2. Merged: "] {
        let done = text.lines().find(|l| l.starts_with(line)).unwrap_or_else(|| panic!("no line '{line}' in {text}"));
        assert!(done.ends_with("Pull request open: none. Blocked: none"), "{done}");
    }
    let (first, second) = (trial.files_on("origin.git"), trial.files_on("origin2.git"));
    assert!(first.contains("Sources/I7/File.swift") && first.contains("Sources/I8/File.swift") && !first.contains("I17"), "{first}");
    assert!(second.contains("Sources/I17/File.swift") && second.contains("Sources/I18/File.swift") && !second.contains("I7/"), "{second}");
    // The simulator belongs to the machine, not to a project: with one
    // slot, verifies of different projects still took turns.
    assert_eq!(trial.verify_overlap().0, 1, "{:?}", trial.events());
    // What the scheduler wrote down while the first verify ran: one issue of
    // each project has a place, and the other of each waits for one.
    let boards = trial.boards();
    let first = &boards[0];
    assert_eq!((first["places"].as_u64(), first["in_order"].as_bool()), (Some(2), Some(false)), "{first}");
    for (project, repo, running, waiting) in [(0, "o/r", 7, 8), (1, "o/r2", 17, 18)] {
        let board = &first["projects"][project];
        assert_eq!(board["repo"], repo, "{first}");
        assert_eq!((board["running"][0]["number"].as_u64(), board["running"].as_array().unwrap().len()), (Some(running), 1), "{first}");
        assert!(board["running"][0]["since"].as_u64().is_some_and(|since| since > 0), "{first}");
        assert_eq!((board["waiting"][0]["number"].as_u64(), board["waiting"][0]["waits_on"].as_str()), (Some(waiting), Some("a free place")), "{first}");
    }
    // By the last verify the first two had merged, and nothing waited any more.
    let last = boards.last().unwrap();
    let merged: usize = (0..2).map(|project| last["projects"][project]["merged"].as_array().unwrap().len()).sum();
    let waiting: usize = (0..2).map(|project| last["projects"][project]["waiting"].as_array().unwrap().len()).sum();
    assert!(merged >= 2 && waiting == 0, "{last}");
}

#[test]
fn in_order_one_project_is_finished_before_the_next_starts() {
    let trial = two_projects("s-order", "");
    let (output, text) = trial.schedule(&["--parallel", "2", "--in-order"]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("one project after the other"), "{text}");
    // Both places went to the first project. The second waited for it.
    let second_starts = text.find("r2#17 c started").unwrap().min(text.find("r2#18 d started").unwrap());
    assert!(text.find("r#7 a ended").unwrap() < second_starts && text.find("r#8 b ended").unwrap() < second_starts, "{text}");
    assert!(text.contains("schedule done for o/r. Merged: ") && text.contains("schedule done for o/r2. Merged: "), "{text}");
    assert!(!text.contains("Blocked: #"), "{text}");
    // While the first project worked, the board said what the second waited on.
    let first = &trial.boards()[0];
    assert_eq!(first["projects"][0]["running"].as_array().unwrap().len(), 2, "{first}");
    let behind: Vec<&str> = first["projects"][1]["waiting"].as_array().unwrap().iter().map(|entry| entry["waits_on"].as_str().unwrap()).collect();
    assert_eq!(behind, ["the project ahead of it", "the project ahead of it"], "{first}");
}

#[test]
fn a_project_never_gets_more_places_than_its_own_limit() {
    // Three places, but the second project may use one.
    let trial = two_projects("s-max", "max_parallel: 1");
    let (output, text) = trial.schedule(&["--parallel", "3"]);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("o/r2: issues labelled 'today', at most 1 at once"), "{text}");
    // 18 started only after 17 had ended, though a place was free.
    assert!(text.find("r2#17 c ended").unwrap() < text.find("r2#18 d started").unwrap(), "{text}");
    let first_end = text.find(" ended after ").unwrap();
    assert!(text.find("r#7 a started").unwrap() < first_end && text.find("r#8 b started").unwrap() < first_end, "{text}");
}

#[test]
fn watching_the_scheduler_picks_up_work_that_arrives_later_and_does_not_end() {
    // Nothing is labelled when it starts.
    let trial = Trial::build("s-watch", &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/I7/", &[], &[])])
        .with_second_project(&github_workflow(2), &[(17, "c", "Sources/I17/", &[], &[])], "");
    let log = trial.dir.join("schedule.log");
    let out = fs::File::create(&log).unwrap();
    let mut child = trial
        .schedule_command(&["--watch"], &[("FACTORY_QUEUE_LOOK_SECONDS", "1")])
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(child.try_wait().unwrap().is_none(), "it ended with nothing to do: {}", fs::read_to_string(&log).unwrap());
    // While it runs, it keeps a board of what it is doing for other
    // processes to read: both projects, and nothing running yet.
    let board = || {
        let files: Vec<PathBuf> = fs::read_dir(trial.dir.join("home/schedulers")).unwrap().map(|entry| entry.unwrap().path()).filter(|path| path.extension().is_some_and(|e| e == "json")).collect();
        assert_eq!(files.len(), 1, "{files:?}");
        fs::read_to_string(&files[0]).unwrap().split_whitespace().collect::<String>()
    };
    let idle = board();
    assert!(idle.contains(&format!("\"pid\":{},", child.id())) && idle.contains("\"watch\":true"), "{idle}");
    assert!(idle.contains("\"repo\":\"o/r\"") && idle.contains("\"repo\":\"o/r2\"") && !idle.contains("\"merged\":[17]"), "{idle}");
    // GitHub stops answering for a while. A scheduler that watches says so
    // once and keeps going: ending would cut off every issue at work.
    fs::write(trial.dir.join("stub/gh.down"), "").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(child.try_wait().unwrap().is_none(), "it ended when GitHub did not answer: {}", fs::read_to_string(&log).unwrap());
    fs::remove_file(trial.dir.join("stub/gh.down")).unwrap();
    // A person labels an issue of the second project.
    fs::write(trial.dir.join("stub/r2/issues.json"), issues_json(&[(17, "c", "Sources/I17/", &["today"], &[])])).unwrap();
    let merged = trial.dir.join("stub/r2/closed-17");
    for _ in 0..300 {
        if merged.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    let still_running = child.try_wait().unwrap().is_none();
    let done = board();
    let _ = child.kill();
    let _ = child.wait();
    let text = fs::read_to_string(&log).unwrap();
    assert!(merged.exists(), "{text}");
    assert!(text.contains("r2#17 c started") && text.contains("r2#17 c ended"), "{text}");
    assert!(text.contains("keeps looking for new work until stopped"), "{text}");
    assert_eq!(text.matches("could not list the issues of o/r:").count(), 1, "{text}");
    assert!(text.contains("o/r answers again"), "{text}");
    // Done with that, it went on watching, and its board says what it did.
    assert!(still_running, "{text}");
    assert!(done.contains("\"merged\":[17]") && done.contains("\"running\":[]"), "{done}");
    assert!(trial.files_on("origin2.git").contains("Sources/I17/File.swift"));
}

#[test]
fn a_watching_scheduler_takes_in_a_project_registered_while_it_runs() {
    let trial = Trial::build("s-follow", &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/I7/", &[], &[])])
        .with_second_project(&github_workflow(2), &[(17, "c", "Sources/I17/", &["today"], &[])], "");
    // Only the first project is registered when it starts.
    let config = trial.dir.join("home/config.yaml");
    let both = fs::read_to_string(&config).unwrap();
    let first_only: String = both.lines().take(2).map(|line| format!("{line}\n")).collect();
    fs::write(&config, &first_only).unwrap();
    let log = trial.dir.join("schedule.log");
    let out = fs::File::create(&log).unwrap();
    let mut child = trial
        .schedule_command(&["--watch"], &[("FACTORY_QUEUE_LOOK_SECONDS", "1")])
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(!trial.dir.join("stub/r2/closed-17").exists(), "o/r2 is not registered yet: {}", fs::read_to_string(&log).unwrap());
    // `factory project add` writes the list. Nobody starts the scheduler again.
    fs::write(&config, &both).unwrap();
    let merged = trial.dir.join("stub/r2/closed-17");
    for _ in 0..300 {
        if merged.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Removed again, it is set aside, and the scheduler goes on.
    fs::write(&config, &first_only).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(3));
    let still_running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    let text = fs::read_to_string(&log).unwrap();
    assert!(merged.exists(), "{text}");
    assert!(text.contains("schedule: 1 project(s)") && text.contains("reading the registered projects again"), "{text}");
    let added = text.find("now also o/r2: issues labelled 'today'").expect(&text);
    assert!(added < text.find("r2#17 c started").expect(&text), "{text}");
    assert!(text.contains("o/r2 is no longer registered: 0 run(s) finish"), "{text}");
    assert!(still_running, "{text}");
}

#[test]
fn an_issue_another_process_is_running_is_left_alone_not_counted_as_blocked() {
    let trial = Trial::build("q-held", &github_workflow(2), ISSUE_BODY)
        .with_issues(&[(7, "a", "Sources/I7/", &["today"], &[]), (8, "b", "Sources/I8/", &["today"], &[])]);
    // Somebody is running issue 7 by hand: they hold its lock.
    let locks = trial.dir.join("home/locks");
    fs::create_dir_all(&locks).unwrap();
    let held = fs::OpenOptions::new().create(true).truncate(false).write(true).open(locks.join("issue-o__r-7.lock")).unwrap();
    held.lock().unwrap();
    let (output, text) = trial.queue(&["--parallel", "2"]);
    drop(held);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("#7 a left alone: another process is running it"), "{text}");
    assert!(text.contains("queue done. Merged: #8. Pull request open: none. Blocked: none"), "{text}");
    let (_, blocked) = trial.cli(&["blocked"], &[]);
    assert!(blocked.contains("no issue is waiting for a person"), "{blocked}");
}
