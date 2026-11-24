# factory

A deterministic task manager. Software owns the loop and every gate. An LLM is called only for bounded steps.

So far it covers repo bootstrap, the plan pipeline, and one issue end to end: acceptance flows, develop, verify, review, pull request, GitHub's checks and review, and the merge, with fix rounds when something fails. A queue runs every ready issue, several at once, and a judgment step decides what happens to an issue that gets stuck. The daemon and the app are not built yet.

## Commands

```bash
factory repo init <owner>/<name> --template <owner>/<template> --collaborator <login>
factory repo preflight <owner>/<name> --collaborator <login>
factory plan validate plans/
factory plan create plans/ --repo <owner>/<name>
factory run <issue> --repo-dir <checkout>
factory queue --repo-dir <checkout>
factory blocked
factory retry <issue> --repo-dir <checkout>
factory drop <issue> --repo-dir <checkout>
factory status
```

| Command | Does | Exit code |
| --- | --- | --- |
| `repo init` | Creates a private repo from a template, then sets branches, rulesets, collaborators, labels and Actions permissions. Ends with a preflight. Safe to rerun | 1 when the preflight fails |
| `repo preflight` | Reads every setting back from GitHub. Changes nothing | 1 when a setting is wrong |
| `plan validate` | Checks plan files and prints the parallel batches. No network | 1 when a plan is invalid |
| `plan create` | Creates issues, parent links and blocked-by links from valid plans. Safe to rerun | 1 when a plan is invalid |
| `run` | Takes one issue through acceptance flows, develop, verify and review, opens a pull request, waits for GitHub on the same commit and merges. `--no-merge` leaves the merge to a person, `--no-pr` stops before pushing | 1 when the issue ends blocked |
| `queue` | Runs every ready issue labelled `today`, most urgent first, up to `--parallel` at once | 1 when an issue ended blocked |
| `blocked` | Lists the issues waiting for a person, with what failed and what the judgment said | |
| `retry` | Starts a stuck issue over: closes the pull request and deletes the branch its last attempt left, then runs it from nothing | 1 when it ends blocked again |
| `drop` | Takes a stuck issue off the queue: closes its pull request, deletes its branch and worktree, removes the `today` label. The issue stays open | |
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

## When an issue is stuck

A run is stuck when a step fails in a way no fix round can help, or when the fix rounds run out. With a `judgment` block in `factory.yaml`, the engine then makes one fresh LLM call, with the issue's whole record, and the LLM picks one action from a fixed menu:

```yaml
judgment: { module: judgment/stuck, vendor: claude }
limits:
  judgments: 2   # new attempts a judgment may start per issue. 3 at most
```

| Action | What the engine does |
| --- | --- |
| `retry` | Throws the attempt away and starts the issue over from nothing, with the same agents |
| `switch-vendor` | Starts over with the vendors swapped, so a different model develops and the other one reviews |
| `split` | Writes the proposed smaller issues as a plan file and opens a pull request for it into `main`. Nothing is created until a person merges that pull request |
| `research` | Not available yet |
| `escalate` | Labels the issue `blocked` and leaves it for a person |

Code decides what an answer may do:

- An action that is not on the menu, or a reply that can't be read, is `escalate`.
- `retry` and `switch-vendor` are refused once the issue has used its new attempts. The LLM can't grant itself more.
- A split goes through the same validator as a plan written by a person. An invalid one goes back to the LLM once, with the validator's errors.
- No action skips a check. A new attempt goes through every gate again.

A new attempt closes the pull request and deletes the branch the earlier attempt left on GitHub. Those are the engine's own.

Without a `judgment` block, a stuck issue goes straight to a person. `factory blocked` lists everything that waits, `factory retry` starts one over, and `factory drop` takes one off the queue.

## The queue

`factory queue` picks work the way people do: by the `today` label (`--label` changes it).

| Rule | Detail |
| --- | --- |
| Ready | The issue was made by `plan create`, is open, carries the label, and everything it is blocked by is closed. GitHub's blocked-by links are the graph |
| Order | `priority:high`, then unlabelled or `priority:normal`, then `priority:low`. Lower issue numbers first within one priority |
| At once | Up to `--parallel` issues, 2 by default. Two issues whose scopes overlap never run together, whatever the plan said |
| Held back | An issue with the `blocked` label waits for a person. An issue the queue already ran is not started twice in one session |

Each issue runs as its own `factory run` process, with its output in `~/.factory/queue/<repo>/`. One that dies takes nothing else with it.

What keeps parallel runs apart:

- **A lock per issue.** A second process for the same issue is refused.
- **The simulator lock.** A module names shared resources in `locks`, and the engine holds them while its command runs. Development happens in parallel. Verify, which needs the simulator, does not.
- **A lock around shared git state.** Fetches, pushes and worktree changes in one checkout take turns.
- **The moved-base check.** When one issue merges while another is still working, the second merges the base branch in and is judged again.

Locks are files under `~/.factory/locks/`, held with the operating system's file lock. The system releases them when a process exits, so a crash leaves none behind.

### Picking up an interrupted run

Before each step the engine writes what the run knows to `progress.json` in the run's folder. When `factory run` finds a run that still says it is running, and no process holds the issue's lock, it picks up at that step:

- The worktree is put back to the engine's last commit. Whatever the interrupted step left half done is discarded, because that step runs again from its start.
- Steps that finished are not repeated. A run cut off while waiting on GitHub goes back to waiting, with the same pull request.
- `--restart` starts over, when nothing was pushed yet.
- A workflow whose steps changed since can't be picked up.

The queue needs nothing special for this: the issue is still open and labelled, so it is picked, and its run resumes.

### Usage limits

A call that fails with signs of a usage or rate limit is not a failed attempt. The engine marks the issue as waiting, sleeps, and makes the same call again: after 1, 5, 15 and 30 minutes, then hourly, for about six hours in all. After that the run stops.

Neither CLI documents a stable format for this, so the engine looks for the phrases both use ("usage limit", "rate limit", "429", "too many requests") in the error and in error lines of the output. It does not read a reset time.

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

Built-in modules are in [modules/](modules/) and ship inside the binary: `agent/acceptance`, `check/acceptance-red`, `agent/dev`, `check/verify`, `agent/review`, `judgment/stuck`. A repo overrides one by adding `modules/<kind>/<name>/` with the same file names on its default branch.

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
| `src/engine/queue.rs` | Which issues are ready, in what order, and how many at once |
| `src/engine/judgment.rs` | What a judgment's answer is allowed to do, and a split as a plan file |
| `src/engine/lock.rs` | Locks that hold across processes |
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
