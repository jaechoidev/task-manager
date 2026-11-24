# factory

A deterministic task manager. Software owns the loop and every gate. An LLM is called only for bounded steps.

So far it covers repo bootstrap, the plan pipeline, and one issue end to end: acceptance flows, develop, verify, review, pull request, GitHub's checks and review, and the merge, with fix rounds when something fails. The queue and the daemon are not built yet.

## Commands

```bash
factory repo init <owner>/<name> --template <owner>/<template> --collaborator <login>
factory repo preflight <owner>/<name> --collaborator <login>
factory plan validate plans/
factory plan create plans/ --repo <owner>/<name>
factory run <issue> --repo-dir <checkout>
factory status
```

| Command | Does | Exit code |
| --- | --- | --- |
| `repo init` | Creates a private repo from a template, then sets branches, rulesets, collaborators, labels and Actions permissions. Ends with a preflight. Safe to rerun | 1 when the preflight fails |
| `repo preflight` | Reads every setting back from GitHub. Changes nothing | 1 when a setting is wrong |
| `plan validate` | Checks plan files and prints the parallel batches. No network | 1 when a plan is invalid |
| `plan create` | Creates issues, parent links and blocked-by links from valid plans. Safe to rerun | 1 when a plan is invalid |
| `run` | Takes one issue through acceptance flows, develop, verify and review, opens a pull request, waits for GitHub on the same commit and merges. `--no-merge` leaves the merge to a person, `--no-pr` stops before pushing | 1 when the issue ends blocked |
| `status` | Shows where each issue stands, from the state database | |

Exit code 2 means the command itself could not run.

All GitHub access goes through `gh api`, so `gh` must be installed and logged in. No feature depends on a particular `gh` version.

## Plan files

The format, the rules for splitting work and the list of checks are in [docs/plan-format.md](docs/plan-format.md).

## Issue bodies

Every issue `factory` creates starts with one machine-readable line:

```
<!-- factory:issue {"v":1,"plan":"todo-v1","key":"add","scope":["Sources/UI/Add/"],"accept":["..."],"flows":[{"file":"add_todo.yaml","new":true}],"accessibility_ids":["todo.add.save"],"depends_on":["todo-v1/shell"]} -->
```

The parent issue carries the same line without `key`. This line is the contract between planning and the engine:

- `plan create` finds existing issues by `plan` and `key`, which is why a rerun creates nothing new.
- `factory run` reads scope, acceptance criteria, flows and accessibility IDs from it.
- Execution order comes from GitHub's native blocked-by links, not from this line.

The rest of the body repeats the same data as markdown for people.

## Running an issue

`factory run <issue>` works on an issue that `plan create` made. It refuses an issue that is closed, is a plan's parent, or is blocked by an issue that is still open.

1. A fresh git worktree on the branch `factory/issue-<n>-<key>`, cut from the default branch.
2. Each step of `factory.yaml`, read from the default branch:
   - **acceptance**: one agent call, by a different vendor from the developer, that writes the Maestro flows the issue adds. The engine commits them as their own commit.
   - **red**: `./verify.sh --red <new flows>`. It passes only when each new flow is valid Maestro and fails on the app as it is, which shows the flow tests something new.
   - **develop**: one headless agent call that may only write inside the worktree.
   - **verify**: the repo's `./verify.sh`, which builds the app and runs every flow. Exit code 0 is the only pass.
   - **review**: one read-only agent call, by a different vendor from the developer.
3. The engine's own steps, which are not in `factory.yaml`:
   - **sync**: if the default branch moved, merge it in and run verify and review again.
   - **pr**: push, and open the pull request with the verified commit, the flows, the verify result and the review verdict in its body.
   - **github**: wait for the checks and the review on the pull request's head commit.
   - **merge**: merge at exactly that commit, then delete the branch and the worktree.

The last two run only when `factory.yaml` has a `github` block. Without it, a run ends with the pull request open.

The acceptance and red steps are skipped for an issue that adds no flow, and a repo without UI flows can leave them out of `factory.yaml`.

What the engine does itself, and no workflow file can turn off:

- Only the engine commits, pushes and opens pull requests. Agents are confined to the worktree and get no git or network commands. See "How agents are confined".
- After acceptance: the writer must have written exactly the flow files the issue names as new, and nothing else. A flow may use only plain Maestro commands: no AI commands and no scripts. Every ID a flow looks for must be in the issue's list, declared by the writer as added, or already in the code.
- After develop: git history must be untouched, `.maestro/` must be untouched, every changed file must be inside the issue's scope, and every accessibility ID must appear as a string literal inside that scope.
- After every step: the branch must still be at the engine's latest commit, with a clean tree.
- The reviewer must name the commit it judged, and it must be that commit.
- Agent output must match its JSON schema. One retry, then the step fails.

### Fix rounds

A failure the developer can fix sends the issue back to the dev step. The developer continues its own session, so it keeps what it learned, and is told what failed. Every step after it then runs again on the new commit: verify first, then a fresh review. No verdict carries over from an earlier commit.

| What failed | The developer is told |
| --- | --- |
| A check, such as verify | The command, its exit code, the end of its output, and where Maestro's failure screenshots are |
| Review asked for changes | The reviewer's summary and its numbered items |
| A gate on the developer's own change: scope, locked flows, missing IDs | Which gate and why. Nothing was committed |
| The acceptance writer rejected a flow change request | The writer's reason |

`on_fail` in `factory.yaml` says where each step's failure goes. Only a return to the dev step is built, so a failed red check still stops the run.

The rounds are limited: 3 by default, set with `limits.fix_rounds` in `factory.yaml`, 5 at most. The limit is shared by every failure before the pull request. Failures found on GitHub have their own limit. When the rounds run out, the run stops and the issue is labelled `blocked`.

Some failures are never retried, because another attempt would not help: an agent whose reply is unusable twice, a developer who changes no files, a check that times out, a reviewer who names the wrong commit, and any change to git history.

Later reviews see what the earlier review asked for, and are told not to raise new matters of taste.

### The GitHub loop

```yaml
github:
  review_check: review   # the check whose verdict comment is the GitHub review
  checks: []             # other checks that must appear before the commit is judged
  wait_minutes: 20       # per commit
  merge: squash          # or merge, or rebase
limits:
  github_fix_rounds: 2   # 5 at most
```

Every signal belongs to one commit. The engine asks GitHub about the pull request's head commit and ignores anything said about another one.

| GitHub says | The engine does |
| --- | --- |
| Every check finished and passed, and the review approved this commit | Merges |
| A check failed | Sends the developer the check's name and the end of its log, as a GitHub fix round |
| The review asked for changes on this commit | Sends the developer the review's summary and items, as a GitHub fix round |
| The review check ended with no verdict for this commit | Stops. No code change fixes that. A missing token secret looks like this |
| Checks are still running after `wait_minutes` | Stops |

A green check is not an approval. The review workflow posts a comment whose first line is `<!-- factory:review {"verdict":...,"sha":...} -->`, and that line is what counts. The engine reads it only from comments by `github-actions[bot]`, which only a workflow in the repository can post as, and only when its `sha` is the head commit. The workflow takes the commit from the event, not from the model.

A GitHub fix round is a full round: the developer continues its session, then verify and the local review run again, and only then is the new commit pushed. GitHub rounds have their own limit, separate from `fix_rounds`.

If the default branch moves during a run, the engine merges it into the issue's branch, which makes a commit nothing has judged, so verify, the local review and GitHub all run again. It uses a merge, not a rebase, so it never force-pushes. A conflict stops the run for a person. This may happen 3 times in one run.

### How agents are confined

| Vendor | Writes | Reads | Shell |
| --- | --- | --- | --- |
| Codex | Its OS sandbox blocks writes outside the worktree | Anywhere on the machine | Inside the sandbox, no network |
| Claude | Its file tools refuse paths outside the worktree | Its file tools refuse paths outside the worktree | A writer may run `maestro check-syntax`. A reader gets none |

Claude has no OS sandbox, so two settings do the work: `permissions.blockWritesOutsideWorkingDirectories` and `permissions.blockReadsOutsideWorkingDirectories`. Allowing the `Write` tool without them lets it write anywhere. A resumed Codex call names its sandbox again, because `codex exec resume` has no `--sandbox` flag.

### Accessibility IDs

The plan names the IDs for the elements an issue adds. The acceptance writer uses them, and may add IDs it needs. Added IDs must follow the naming rule in the repo's `AGENTS.md`. The engine comments them on the issue and hands the full list to the developer.

### Flow change requests

The developer may not edit a flow. When it thinks one is wrong, it says so in `flow_change_request` in its report. The engine then asks the acceptance writer to rule:

- **Accept**: the writer edits the flow. It may change only the flows the issue lists, and only use IDs that are in the code. The engine commits the change as its own commit, and verify and review judge that commit.
- **Reject**: the code must change. The issue goes back to the developer with the writer's reason, as a fix round.

A run that ends blocked records why, labels the issue `blocked` and keeps the worktree.

Files live under `~/.factory` (or `FACTORY_HOME`):

| Path | Holds |
| --- | --- |
| `state.db` | SQLite: one row per issue, one row per step |
| `requests.jsonl` | One line per agent call, check or gate |
| `runs/<repo>/issue-<n>/<run-id>/` | Prompts, schemas, replies, logs and the PR body of one run. Files from the second time the run goes back carry `-r2` in their names |
| `worktrees/<repo>/issue-<n>/` | The issue's worktree. Removed after a merge, kept when the run ends blocked |

## Modules

A step in `factory.yaml` names a module. A module is a folder with a `module.yaml`:

```yaml
name: verify
kind: check              # agent | check
needs: [worktree, head_sha]
produces: [verify_log]
run: ./verify.sh
pass_when: exit_code == 0
timeout_minutes: 20
locks: [simulator]
```

Built-in modules are in [modules/](modules/) and ship inside the binary: `agent/acceptance`, `check/acceptance-red`, `agent/dev`, `check/verify`, `agent/review`. A repo overrides one by adding `modules/<kind>/<name>/` with the same file names on its default branch.

A check's `run` may contain `{{new_flows}}`, which becomes the paths of the flows the issue adds.

Before a run, the engine checks that every `needs` is produced by an earlier step, that every `on_fail` target exists, that an acceptance step comes before the dev step, and that the acceptance writer and the reviewer each use a different vendor from the developer.

### What a repo's `verify.sh` must do

| Call | Passes (exit 0) when |
| --- | --- |
| `./verify.sh` | The build and every flow in `.maestro/` pass |
| `./verify.sh --red FLOW...` | Each named flow is valid Maestro, runs, and fails |

The template repo's `verify.sh` does both for an Xcode project and an iOS simulator. It needs Maestro (`brew install mobile-dev-inc/tap/maestro`) and Java 17 or newer.

## Build and test

```bash
cargo test
```

```bash
cargo install --path .
```

## Layout

| Path | Holds |
| --- | --- |
| `src/plan/model.rs` | The plan.yaml format and loading |
| `src/plan/validate.rs` | Every check, and the batch computation |
| `src/plan/render.rs` | Text, markdown and JSON reports |
| `src/plan/create.rs` | Issue bodies, creation and links, behind a `Github` trait |
| `src/repo.rs` | `repo init` and the preflight |
| `src/engine/mod.rs` | The steps and the fixed gates |
| `src/engine/rounds.rs` | The loop: what runs next, and when the rounds run out |
| `src/engine/github.rs` | Checks, the review verdict and the merge, each tied to one commit |
| `tests/fix_loop.rs` | The real engine run against stand-ins for `gh`, `codex` and `claude` |
| `src/engine/flows.rs` | What the engine checks in a flow file without running it |
| `src/engine/workflow.rs` | `factory.yaml` and `module.yaml` loading and checks |
| `src/engine/agent.rs` | Headless calls to `codex exec` and `claude -p` |
| `src/engine/state.rs` | The SQLite state and the request log |
| `src/engine/proc.rs` | Child processes with a time limit and group kill |
| `src/engine/git.rs` | Worktrees, commits, diffs |
| `modules/` | Built-in module definitions, prompts and schemas |
| `src/gh.rs` | The `gh api` wrapper |
| `src/labels.rs` | The label set shared by `repo init` and the validator |
