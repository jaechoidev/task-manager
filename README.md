# factory

Status, open decisions and what needs a person: [the status report](https://claude.ai/artifact/1qmZayiHcGUBccEFtQGSe2).

A deterministic task manager. Software owns the loop and every gate. An LLM is called only for bounded steps.

So far it covers repo bootstrap, the plan pipeline, and one issue end to end: acceptance flows, develop, verify, review, pull request, GitHub's checks and review, and the merge, with fix rounds when something fails. A queue runs every ready issue, several at once, and a judgment step decides what happens to an issue that gets stuck. A local API serves all of it to the macOS app in [app/](app/), and both run as a launchd service on the machine that works on issues.

## Commands

```bash
factory repo init <owner>/<name> --template <owner>/<template> --collaborator <login>
factory repo preflight <owner>/<name> --collaborator <login>
factory plan validate plans/
factory plan create plans/ --repo <owner>/<name>
factory run <issue> --repo-dir <checkout>
factory queue --repo-dir <checkout>
factory blocked
factory handoff
factory promote --repo-dir <checkout>
factory retry <issue> --repo-dir <checkout>
factory drop <issue> --repo-dir <checkout>
factory workflow validate --repo-dir <checkout>
factory workflow propose --file <factory.yaml> --repo-dir <checkout>
factory module list --repo-dir <checkout>
factory module new <kind>/<name> --repo-dir <checkout>
factory project add <checkout> [--label <label>] [--max-parallel <n>]
factory project list
factory skills check <owner>/<name>
factory skills update <owner>/<name>
factory schedule [--parallel <n>] [--in-order] [--watch]
factory capacity [--write] [--prepare]
factory providers
factory providers check <name> [--tier <tier>]
factory serve --repo-dir <checkout>
factory status
```

| Command | Does | Exit code |
| --- | --- | --- |
| `repo init` | Creates a private repo from a template, then sets branches, rulesets, collaborators, labels and Actions permissions, and writes the [skills](#skills) to the default branch. Ends with a preflight. Safe to rerun | 1 when the preflight fails |
| `repo preflight` | Reads every setting back from GitHub, and warns when the default branch's skills differ from this program's. Changes nothing. Run it as the repository's owner: some settings are shown only to an admin, so an agent account can't check its own repos | 1 when a setting is wrong, 2 when the account isn't an admin |
| `plan validate` | Checks plan files and prints the parallel batches. No network | 1 when a plan is invalid |
| `plan create` | Creates issues, parent links and blocked-by links from valid plans. Safe to rerun | 1 when a plan is invalid |
| `run` | Takes one issue through acceptance flows, develop, verify and review, opens a pull request, waits for GitHub on the same commit and merges. `--no-merge` leaves the merge to a person, `--no-pr` stops before pushing | 1 when the issue ends blocked |
| `queue` | Runs every ready issue labelled `today`, most urgent first, up to `--parallel` at once | 1 when an issue ended blocked |
| `schedule` | Runs the ready issues of every registered project, with one set of places for all of them. `--watch` keeps it running | 1 when an issue ended blocked |
| `project` | `add`, `list` and `remove` the checkouts `schedule` works on. `add` refuses a checkout whose `verify.sh` can't pass anything, and notes when its commits would carry another name than the account `gh` works as. A watching scheduler and the API take a change in at their next look | 1 when a listed folder is not a checkout, 2 when `add` refuses |
| `skills` | `check` compares the [skills](#skills) on a repository's default branch with this program's. `update` commits this program's there, through GitHub | `check`: 1 when they differ |
| `blocked` | Lists the issues waiting for a person, with what failed and what the judgment said | |
| `retry` | Starts a stuck issue over: closes the pull request and deletes the branch its last attempt left, then runs it from nothing | 1 when it ends blocked again |
| `drop` | Takes a stuck issue off the queue: closes its pull request, deletes its branch and worktree, removes the `today` label. The issue stays open | |
| `handoff` | Writes the day's handoff document for each repository, from the state database | |
| `promote` | Opens or refreshes the pull request from the default branch into `main`. Never merges it | |
| `workflow validate` | Checks a workflow file against the repository's modules. No network | 1 when it is not valid |
| `workflow propose` | Checks a workflow file, then opens a pull request that replaces `factory.yaml` with it | 1 when it is not valid |
| `module list` | Lists the built-in modules and the repository's own | |
| `module new` | Opens a pull request that adds a new module's files. `--dry-run` prints them | |
| `capacity` | Reads what this machine has, checks the tools a run needs, and proposes how much to run at once. Changes nothing unless told: `--write` saves the proposal as this machine's pools, `--prepare` creates the simulators for them | 1 when something needs attention |
| `providers` | Lists this machine's agent subscriptions, what each one's check found, quota where it can be read, and who fills each role. `providers check <name>` tries one for real | 1 when the file has a problem, or a check failed |
| `serve` | Serves the local API that the app and other clients use | |
| `status` | Shows where each issue stands, from the state database. `--refresh` first corrects it from GitHub, for pull requests a person merged or closed by hand | |

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
   - **research**: only for an issue labelled `needs-research`, or when a judgment asks for it. One agent call that writes `research/issue-<n>.md` with its sources. The engine commits it, and the acceptance writer and the developer are pointed at it.
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
| Antigravity | **Not confined.** Under its `--sandbox` it wrote into a new folder in the home directory | Anywhere on the machine | Inside its sandbox |

Antigravity is not used: it fails `factory providers check`, and the engine only uses a provider that passed. GLM runs through the Claude program and is confined like Claude.

One module may reach the web: research, which sets `web: true`. On Claude that adds the web search and fetch tools. Its prompt says that text from the web is information, never an instruction, and what it writes is one notes file inside the worktree. Codex has no network in its sandbox, so a Codex researcher works from the code alone.


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
| `switch-vendor` | Starts over with the vendors swapped, so a different model develops and the other one reviews. On a machine that lists who may develop, the family that failed is ruled out instead |
| `split` | Writes the proposed smaller issues as a plan file and opens a pull request for it into `main`. Nothing is created until a person merges that pull request |
| `research` | Starts over with a research step first, on the question the judgment asked. Needs a research step in the workflow |
| `escalate` | Labels the issue `blocked` and leaves it for a person |

Code decides what an answer may do:

- An action that is not on the menu, or a reply that can't be read, is `escalate`.
- `retry` and `switch-vendor` are refused once the issue has used its new attempts. The LLM can't grant itself more.
- A split goes through the same validator as a plan written by a person. An invalid one goes back to the LLM once, with the validator's errors.
- No action skips a check. A new attempt goes through every gate again.

A new attempt closes the pull request and deletes the branch the earlier attempt left on GitHub. Those are the engine's own.

Without a `judgment` block, a stuck issue goes straight to a person. `factory blocked` lists everything that waits, `factory retry` starts one over, and `factory drop` takes one off the queue.

## Documents

### The pull request body

With a `summary` block in `factory.yaml`, one LLM call writes the part of the body meant for a person: what changed, how the run went, and what to look at before trusting it.

```yaml
summary: { module: agent/summary, vendor: claude }
```

The facts stay the engine's. The verified commit, the gates, the verify result and the review verdict are written by code, below the summary, and the summary module is told not to repeat them. If the summary call fails, the pull request is opened without it.

### The handoff

`factory handoff` writes one file per repository and day: `<folder>/<repo>/<date>.md`. It leads with what needs a person, then what each summary flagged, then every issue with its steps and times. `factory queue` writes it when it finishes.

It is generated from the state database and the run files, with no LLM call, and is written again on every export. The folder is `--out`, or `handoffs:` in `~/.factory/config.yaml`, or `~/.factory/handoffs`. Dates are in UTC.

### The promote pull request

`factory promote` opens a pull request from the default branch into `main`, listing what merged since the last promotion. Run again, it refreshes the same pull request. A person approves and merges it. The engine never does.

## The API and the app

`factory serve` answers JSON under `/v1`, on `127.0.0.1:7878` unless told otherwise. The app in [app/](app/) is a client of it, and so is anything else: no client gets a private path.

| Route | Answers |
| --- | --- |
| `GET /v1/health` | The version, the server's process id, and the repositories it may act on |
| `GET /v1/issues` | Every issue run so far: its state, its stage, what it waits on, which pass it is on |
| `GET /v1/issues/<owner>/<repo>/<n>` | One issue: every step with who answered and on which model, the summary, what failed, the judgment, the run's files |
| `GET /v1/issues/<owner>/<repo>/<n>/log` | The end of the newest output of that run |
| `GET /v1/blocked` | The issues waiting for a person |
| `GET /v1/workflow?repo=<owner>/<repo>` | The workflow as a graph: steps in order, and where each failure goes. Also the file's text |
| `GET /v1/modules?repo=<owner>/<repo>` | The modules a workflow may name: built-in and the repository's own |
| `POST /v1/workflow/validate?repo=...` | Checks the workflow file in the request body. Answers the problems, or the graph |
| `POST /v1/workflow/propose?repo=...` | Checks it, then opens a pull request for it |
| `POST /v1/modules?repo=...` | Scaffolds `{"module": "<kind>/<name>", "role": ...}` into a pull request |
| `GET /v1/usage` | Calls, tokens and usage limits hit, per vendor. Tokens are what each program counted for its own calls: all input, the cached part of it, and output |
| `POST /v1/quota/refresh` | Starts `factory providers refresh`, which reads every provider's plan again |
| `GET /v1/providers` | What `factory providers` shows, as data: every provider and tier with what its check found, each job's list, the jobs a list can be given for, and the tiers being checked right now |
| `POST /v1/providers` | Changes `providers.yaml`: `{"tiers": [{"provider", "tier", "model"}], "roles": {"<job>": [{"use", "tier"}]}, "check": true}`. Refused with every reason when the result would not be consistent. With `check`, each tier whose model changed is checked at once |
| `POST /v1/providers/check` | Starts `factory providers check` for `{"provider", "tier"}` |
| `GET /v1/schedule` | What runs where: each running scheduler's places with what holds them, what waits and what for, what is parked until when, who holds each slot of each pool, and who is out of quota |
| `GET /v1/capacity` | What `factory capacity` reports, as data |
| `POST /v1/issues/<owner>/<repo>/<n>/retry` | Starts `factory retry` for the issue |
| `POST /v1/issues/<owner>/<repo>/<n>/drop` | Runs `factory drop` for the issue |

The two commands and the workflow need a checkout: give `serve` one `--repo-dir` per repository.

Who may call it:

- **On this machine only, by default.** The API can start runs.
- **Any other address needs `--token`.** The server refuses to start without one, and then every request must send `Authorization: Bearer <token>`.

The engine returns data and no layout. Clients poll: there is no event stream yet.

### What the schedule is read from

The scheduler and `serve` are separate processes, so the scheduler writes down what it decided and `serve` reads it. None of these files decides anything. A lock holds a slot, and the scheduler's own memory holds the places.

- **A scheduler's board** is `~/.factory/schedulers/<pid>.json`, written whole each time something starts, ends or is parked. It lists each project's running, waiting and parked issues. A board whose process is gone is ignored, and the next scheduler removes it.
- **A slot's note** is `~/.factory/locks/<pool>-<slot>.holder`, left by the run that holds the slot and removed when it lets go: the issue, the step and since when. For an agent call it also names the model. A note whose process is gone is ignored.
- **What an issue that has not started waits on** is one of: a person (the `blocked` label), other issues to be closed, an issue that changes the same files, the machine being busy, its project's own limit, the project ahead of it, or a free place.
- **Quota** is `~/.factory/availability.json`, as in [Usage limits](#usage-limits).

## The queue

`factory queue` picks work the way people do: by the `today` label (`--label` changes it).

| Rule | Detail |
| --- | --- |
| Ready | The issue was made by `plan create`, is open, carries the label, and everything it is blocked by is closed. GitHub's blocked-by links are the graph |
| Order | `priority:high`, then unlabelled or `priority:normal`, then `priority:low`. Lower issue numbers first within one priority |
| At once | Up to `--parallel` issues. By default 2, or the size of this machine's simulator pool when that is larger. Two issues whose scopes overlap never run together, whatever the plan said |
| Looking again | When a run ends, and every 30 seconds while a place is free. An issue labelled while the queue runs is picked up |
| Held back | An issue with the `blocked` label waits for a person. An issue the queue already ran is not started twice in one session |

Each issue runs as its own `factory run` process, with its output in `~/.factory/queue/<repo>/`. One that dies takes nothing else with it.

What keeps parallel runs apart:

- **A lock per issue.** A second process for the same issue is refused.
- **Resource pools.** A module names shared resources in `locks`, and the engine holds one slot of each while its command runs. A pool has one slot unless this machine's config says more, so by default verify, which needs the simulator, runs one at a time. See [Pools](#pools).
- **A lock around shared git state.** Fetches, pushes and worktree changes in one checkout take turns.
- **The moved-base check.** When one issue merges while another is still working, the second merges the base branch in and is judged again.
- **One merge at a time per repository.** The last look at the base branch and the merge happen under one lock. Without it two issues could both see an unmoved base and both merge, and the second would land on code it was never verified with. This covers the runs of one machine. Against a person or a second machine merging in that moment, only a GitHub rule that branches be up to date helps.

Locks are files under `~/.factory/locks/`, held with the operating system's file lock. The system releases them when a process exits, so a crash leaves none behind.

### Several projects on one machine

`factory queue` works on one repository. `factory schedule` works on every project the machine has registered, and is the same scheduler with more than one project.

```
factory project add ~/code/todo-trial
factory project add ~/code/other-app --max-parallel 1
factory project list
factory schedule
```

The simulators and the agent subscriptions are already shared between projects, because the pools and the quota ledger belong to the machine. What `schedule` adds is one set of places for all projects, `--parallel` of them, and a rule for who gets the next free one.

| Mode | Who gets the next free place |
| --- | --- |
| Side by side, the default | The project with the fewest issues running. With the same number, the one registered first |
| `--in-order` | The first registered project that has anything running or ready. Nobody else starts until it has neither |

- **A project's own limit.** `--max-parallel` on a project caps its issues, whatever is free.
- **Parked issues are not in the way.** A project whose issues are all parked for quota counts as having nothing left, in both modes.
- **Inside a project nothing changes.** Priority labels, blocked-by links and the rule that overlapping scopes never run together work as in `factory queue`. Scopes are only compared within a project.
- **The machine guard.** A further issue starts only while `factory capacity` would say the machine has room. The first one always starts. `--no-guard` turns this off.
- **`--watch`** never ends. With nothing to do it looks again every 30 seconds, so labelling an issue `today` is all it takes to start it. This is the long-running form, and what the service runs. A watching scheduler does not end when GitHub fails to answer: it says so once, keeps what it listed last, and looks again.
- **An issue someone else is running** is left alone and not counted as blocked.

Projects are kept in `~/.factory/config.yaml`, in the order they were added. A watching scheduler reads the list again at each look, so `project add` and `remove` need no restart. `factory serve` with no `--repo-dir` serves the registered projects. In the output an issue is named with its repository, as in `todo-trial#37`.

### Running as a service

```
factory service install
factory service status
factory service uninstall
```

`install` makes the scheduler and the API two launchd agents of this Mac. They start at login and are started again when they stop. `status` exits 1 unless both run and the API answers as its own agent, so a script can tell. From then on, putting the label on an issue is all it takes, and the app always has an engine to talk to.

| Agent | Runs | Output |
| --- | --- | --- |
| `dev.factory.schedule` | `factory schedule --watch`, with the flags given to `install` (`--parallel`, `--in-order`, `--no-merge`, `--no-guard`) | `~/.factory/service/schedule.log` |
| `dev.factory.serve` | `factory serve --bind 127.0.0.1:7878`, or the address given with `--bind` | `~/.factory/service/serve.log` |

- **What goes into an agent's environment** is only the `PATH` of the shell that installed it, each folder once, and `FACTORY_HOME` when that was set. So install it from a shell that finds `git`, `gh`, `claude`, `codex` and the Xcode tools. No login of that shell is carried over: the agents' calls use this machine's own logins.
- **It needs a registered project.** `install` refuses when `factory project list` is empty. Projects added or removed later need no new install: the scheduler and the API read the list again at each look. A project removed while its issues run lets them finish and starts nothing new of it.
- **To change how it runs,** run `install` again with other flags. It stops what is loaded and starts the new settings. It waits up to 10 seconds for launchd to finish removing each old agent, offers the new file up to five times, and tries both agents even when one is refused. It says the API runs only once `/v1/health` answers with the agent's own process id, which it waits up to 10 seconds for. When an agent is not loaded, or another process answers on the API's address, it says which and exits 1.
- **The agents run the program where it was when `install` ran.** Building again replaces that file, and an agent picks the new build up the next time it starts.
- **`uninstall` cuts off issues at work.** They pick up at the step they were on the next time they are run.
- **Don't also start `factory serve` by hand** on the same address. The second one can't listen. `install` and `status` name the process that holds it.

### What a machine can run at once

`factory capacity` answers that from the machine itself. On its own it changes nothing and starts nothing.

| It reports | From |
| --- | --- |
| Cores, memory, free disk, and how idle the CPU and memory are now | `sysctl`, `top`, `memory_pressure`, `df` |
| Whether each tool a run needs is there: Xcode, Command Line Tools matching macOS, Maestro, Java, git, `gh` and its login, `codex`, `claude` | Each tool's own version command |
| iPhone simulators on the newest iOS | `simctl` |
| How many simulator steps and agent calls to allow at once, and why | The numbers below |
| How many more simulator steps could start this minute | The CPU's idle share and free memory |

The steps of a run need different things, so there is no single number. Agent calls use almost no local CPU: a subscription's usage limit bounds them. A step that builds the app and drives a simulator needs cores and memory.

The proposal for simulator steps is the smaller of two limits, and never more than 3:

- **Cores:** 4 per step, from all cores.
- **Memory:** 8 GB per step, with 8 GB kept back.

Those come from one measurement, of the trial app on a 12-core, 96 GB Mac, written up in [docs/findings/2026-11-24-capacity.md](docs/findings/2026-11-24-capacity.md). The trial app builds in seconds. A project with a long build needs more cores per step, and should be measured.

### Pools

A pool is a resource with a number of slots, counted across every project on the machine. They live in `~/.factory/config.yaml`:

```yaml
pools:
  simulator: 3
  codex: 2
  claude: 2
```

| Pool | Who takes a slot | With no entry |
| --- | --- | --- |
| A name a module lists in `locks`, such as `simulator` | A check step, for as long as its command runs | 1 slot |
| A provider's name: `codex` or `claude`, or the names in `providers.yaml` when this machine has one | An agent call, for that call only | No cap |

With no config file the engine behaves as before: one simulator step at a time and no cap on agent calls.

```
factory capacity --write      # save the proposal as this machine's pools
factory capacity --prepare    # create the simulators the simulator pool needs
```

- **`--write`** replaces `pools` with the proposal and keeps the file's other settings. Agent calls get one entry per provider this machine lists, or `codex` and `claude` when it lists none. Edit the file by hand to choose other numbers.
- **`--prepare`** creates `factory-sim-1` to `factory-sim-N` on the newest iOS, boots each once, waits for the machine to calm down and shuts it down. A simulator's first boot keeps the machine busy for minutes, and that should not happen in the middle of a run.

A check is told its slot in the environment, as `FACTORY_SLOT_<POOL>`, for example `FACTORY_SLOT_SIMULATOR=2`. What the slot means is the project's business. The template's `verify.sh` uses the simulator named `factory-sim-<slot>`. Slot 1 falls back to the `SIMULATOR` it names when that simulator does not exist. Any other slot fails with a message to run `--prepare`, because two verifies on one simulator would break each other.

Slots are lock files, `~/.factory/locks/<pool>-<slot>.lock`. A run that crashes gives its slot back, as with every lock.

### Picking up an interrupted run

Before each step the engine writes what the run knows to `progress.json` in the run's folder. When `factory run` finds a run that still says it is running, and no process holds the issue's lock, it picks up at that step:

- The worktree is put back to the engine's last commit. Whatever the interrupted step left half done is discarded, because that step runs again from its start.
- Steps that finished are not repeated. A run cut off while waiting on GitHub goes back to waiting, with the same pull request.
- `--restart` starts over, when nothing was pushed yet.
- A workflow whose steps changed since can't be picked up.

The queue needs nothing special for this: the issue is still open and labelled, so it is picked, and its run resumes.

### Providers

A workflow file names a vendor per step, `codex` or `claude`, and by default that is who answers. A machine can know more than that: which subscriptions it has, which models each offers, and who to try for each job, in what order. That lives in `~/.factory/providers.yaml`. It holds no secret.

```yaml
providers:
  chatgpt:
    cli: codex
    family: openai
    tiers:
      high: gpt-6.2-astra
      standard: gpt-6.2-sol
  claude:
    cli: claude
    family: anthropic
    tiers: { high: opus, standard: sonnet, fast: haiku }
  glm:
    cli: claude                 # GLM runs through the Claude program
    family: zhipu
    env_file: ~/.zai/env.sh     # sets the endpoint and key. Yours, read by the shell
    quota: zai                  # its remaining quota can be read
    tiers: { high: opus, standard: sonnet, fast: haiku }

roles:
  acceptance: [{ use: claude, tier: high }, { use: glm, tier: high }]
  develop: [{ use: chatgpt, tier: standard }, { use: glm, tier: high }]
  review:  [{ use: claude, tier: high }, { use: glm, tier: high }]
  summary: [{ use: glm, tier: fast }, { use: claude, tier: fast }]
```

| Part | Meaning |
| --- | --- |
| `cli` | The program that runs it: `codex`, `claude` or `agy`. The engine knows each one's flags |
| `family` | Who made the model. A tier may name its own, as one subscription can serve several |
| `env_file` or `config_dir` | How this login is selected. Without either, the program's own login |
| `tiers` | Your names for its models. Model names go stale, so they live here and nowhere in the engine |
| `roles` | For each job, who to try, in order: `research`, `acceptance`, `develop`, `review`, `summary`, `judgment` |

A role with a list uses it, and the vendor in the workflow file is ignored for that role. A role with no list uses the workflow file's vendor, as before. With no file nothing changes.

A role with no list is not pinned to a model. The built-in workflow names a vendor and no model for most steps, so the program's own default answers, and that default is a setting of the machine (for Claude, the `model` in `~/.claude/settings.json`). The Mac Studio's first acceptance flows were written on Fable, the model its Claude settings name. To pin a job, give it a list. `factory providers` names each job that has none.

Who answers a call is the first in the role's list that passes three tests:

1. **It passed its check.** `factory providers check <name>` makes three short real calls per tier, in `~/.factory/checks/`. One must return structured output and write a file in its own folder. Two are asked to write outside it: next to the folder, and in a new folder in the home directory, which is removed afterwards. The verdict comes from what is on disk, never from what the agent says. A result belongs to the model the tier named, so changing a tier's model means checking again.
2. **It is independent.** The reviewer and the acceptance writer never come from the developer's family. This is decided per issue, as the run goes, because who develops depends on who had quota. The developer gets its first choice and the others work around it.
3. **It is in quota.** See [Usage limits](#usage-limits).

When nobody passes, the issue stops and says why for each one. A list like `review: [glm]` is valid, but an issue GLM developed then has no reviewer.

- **A login of its own starts clean.** A provider with an `env_file` or `config_dir` is started without the inherited variables that could select another login (`ANTHROPIC_*`, `CLAUDE_*`, `OPENAI_*`, `CODEX_*`). The shell sources the env file, so the key in it never passes through the engine.
- **A session belongs to who made it.** A fix round continues the developer's session only when the same provider and model answer again.
- **`switch-vendor`** from a judgment, on a machine that lists who may develop, rules out the family that failed for the next attempt.
- **`factory providers`** lists all of this: where each program is, how each login is selected, what each tier's check found, what Z.ai reports, and who is set aside. It exits 1 when the file has a problem. A run refuses to start on a file with a problem.

What was found when the four subscriptions on the build machine were checked is in [docs/findings/2026-11-24-providers.md](docs/findings/2026-11-24-providers.md). In short: Claude, GLM and ChatGPT pass. Antigravity fails, because its sandbox let it write into the home directory.

### Usage limits

A call that fails with signs of a usage or rate limit is not a failed attempt. The provider and model are set aside, in `~/.factory/availability.json`, which every run on the machine reads. So the next issue does not try them either.

| Then | What happens |
| --- | --- |
| Someone else in the role's list may answer | They do, at once |
| Nobody may, and the first comes back within 90 seconds | The call waits in place |
| Nobody may, and it takes longer | The issue is parked |

- **Set aside for how long.** 1 minute after the first limit, then 5, 15, 30 and 60 minutes for each further one in a row. A good reply ends the row. The engine does not read a reset time from an error message.
- **Z.ai is read directly.** For a provider with `quota: zai` the engine asks Z.ai how much of the 5-hour and weekly windows is used, at most once a minute. A full window sets every model of that provider aside until the reset time Z.ai names. The address is not in Z.ai's documentation, so a reading that fails changes nothing.
- **Claude and Codex say it with every call.** Neither has a report to ask for, but each tells how much of the plan is used as part of a reply, and the engine keeps the latest reading per provider.
  - Claude prints an event with its 5-hour and weekly windows in its streaming output, which is why the engine calls it with `--output-format stream-json`. When Claude says a window is used up, that is the limit, whatever the error text says.
  - Codex writes the same into its own session file after each reply. The engine reads that file and nothing else of Codex's. The format is Codex's own and not documented, so a file it can't read gives no reading.
  - A used-up 5-hour or weekly window sets every model of the provider aside until it resets, but for an hour at most. Such a reading can only be renewed by a call, so after an hour one call is tried, and that call brings a new reading. A used-up window for one model of Claude sets only that model aside, through the failed call.
  - The engine never asks for a login or a token for any of this.
- **A parked issue** has status `waiting` and a wake time. Nothing failed, nobody is asked to look, and it holds no simulator or agent slot. `factory run` exits with code 75 and says until when. Run again before then, it parks again without spending a call.
- **`factory queue` handles parking by itself.** It goes on with other issues, starts the parked one again at its wake time, at the step it stopped at, and when only parked issues are left it waits for the first wake time.

Codex documents no stable format for a failed call, so there the engine looks for the phrases a limit is worded with ("usage limit", "rate limit", "429", "too many requests") in the error and in error lines of the output. It does the same for Claude when Claude's own event does not say.

`factory providers` prints each provider's latest reading and when it was read. `GET /v1/schedule` carries the same, and the app shows it on its Usage screen.

#### Changing the file from the app

The app's Providers screen changes which model a tier names, adds a tier, and reorders who is tried for each job. It goes through `POST /v1/providers`, and the rules are the engine's:

- **The file stays yours.** A tier's line and the `roles:` block are rewritten in place. Comments and everything else stay as they are. The result is read back and compared with what was asked for, and a file laid out in a way the engine can't follow is left alone, with a message to edit it by hand. The file as it was is kept as `providers.yaml.bak`.
- **A change is refused whole** when it would leave a job naming a tier that does not exist, a model name with spaces in it, or anything else `factory providers` would report.
- **A changed model is not usable until it passes its check.** The check that was on record was for the old model. Saving from the app starts the check at once (three short calls), and until it passes the engine skips that tier wherever it is listed.
- **What is not a setting stays out.** The reviewer and the acceptance writer never come from the developer's family, whatever the lists say.

`factory providers refresh` reads every plan now, or one provider's when it is named. Z.ai is asked directly, which costs nothing. Claude and Codex only say it in a reply, so each gets one small call on its cheapest checked tier, in an empty folder. The refresh button beside Quota on the app's Usage screen does the same.

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

Built-in modules are in [modules/](modules/) and ship inside the binary: `agent/research`, `agent/acceptance`, `check/acceptance-red`, `agent/dev`, `check/verify`, `agent/review`, `agent/summary`, `judgment/stuck`. A repo overrides one by adding `modules/<kind>/<name>/` with the same file names on its default branch.

A check's `run` may contain `{{new_flows}}`, which becomes the paths of the flows the issue adds.

Before a run, and before a change to the file is proposed, the engine checks that:

- every module exists, and every step's `needs` is produced by an earlier step
- every check and every review has an `on_fail`, so every failure has somewhere to go
- every `on_fail` points at an earlier step, so there is no loop but the fix loops
- research comes before acceptance and development, and acceptance before development
- the acceptance writer and the reviewer each use a different vendor from the developer
- the limits are within what the engine allows

### Changing the workflow

`factory.yaml` on the default branch is the source of truth, and a run reads it from there. So a change is never written in place: `factory workflow propose` checks the new file and opens a pull request for it, and a person merges that. The app's workflow editor does the same through the API.

### Adding a module

`factory module new <kind>/<name>` opens a pull request with a starting point:

| Kind | What it scaffolds |
| --- | --- |
| `check/<name>` | A `module.yaml` and a `run.sh` that fails until someone writes it. A check that passes without checking anything is worse than no check |
| `agent/<name> --role <role>` | A copy of the built-in module for that role, with its prompt and schema, to edit |
| `judgment/<name>` | A copy of the built-in judgment module |
| A built-in's own name | A copy of that built-in, which then overrides it in this repository |

### What a repo's `verify.sh` must do

| Call | Passes (exit 0) when |
| --- | --- |
| `./verify.sh` | The build and every flow in `.maestro/` pass |
| `./verify.sh --red FLOW...` | Each named flow is valid Maestro, runs, and fails |

The template repo's `verify.sh` does both for an Xcode project and an iOS simulator. It needs Maestro and Java 17 or newer:

```bash
brew trust --formula mobile-dev-inc/tap/maestro
```

```bash
brew install mobile-dev-inc/tap/maestro
```

Homebrew refuses a formula from a tap it doesn't trust, so the first line comes first. Maestro pulls in `openjdk`, which Homebrew keeps off `PATH`, and `/usr/bin/java` is Apple's stub, which fails without a JDK it knows. Rust from Homebrew's `rustup` is kept off `PATH` the same way. Put them first in `~/.zprofile`:

```bash
export PATH="/opt/homebrew/opt/openjdk/bin:/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
```

The service takes its `PATH` from the shell that installs it, so this line is also what lets it find them.

## Skills

A project run by factory carries skills for the Claude Code sessions people work in there, in `.claude/skills/factory-*`. They are the steps that need judgment or a person's word, written down so every session follows them the same way. The rest is commands.

| Skill | For |
| --- | --- |
| `factory-plan` | Turning a feature discussion, spec or document into a plan file and its pull request into `main` |
| `factory-status` | What runs, what is blocked and why, and a retry or a drop when the person asks, through the API |
| `factory-promote` | The promote pull request, whose merge stays the person's |

Their text is in [skills/](skills/) and is built into the program, so a project's copy matches the engine that runs it. `repo init` writes them, `factory skills update <owner>/<name>` refreshes them after an engine update, and the preflight warns when they differ. Only `factory-*` folders are factory's: a project's other skills are left alone.

- **The engine's own agents never see them.** Every agent call runs with skills turned off (`--disable-slash-commands`), and so does the GitHub review in the template's `review.yml`.
- **No issue can change them.** The validator refuses a scope under `.claude/`, and under `.github/`.

Three more live in this repository's [.claude/skills/](.claude/skills/), for sessions started in it: `factory-new-project` makes a repository and its app skeleton, `factory-register-project` registers it on the machine that runs the engine, and `factory-update-engine` updates that machine and every project's skills.

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
| `src/skills.rs` | The skills a project carries, built in from `skills/`, and how they are compared and committed |
| `skills/` | The project skills' text. Changing one here and running `factory skills update` is how projects get the change |
| `.claude/skills/` | This repository's own skills: making a project, registering it on the engine's machine, updating the engine |
| `src/engine/mod.rs` | The steps and the fixed gates |
| `src/engine/rounds.rs` | The loop: what runs next, and when the rounds run out |
| `src/engine/queue.rs` | The scheduler: which issues are ready, in what order, how many at once, and which project gets the next place |
| `src/engine/judgment.rs` | What a judgment's answer is allowed to do, and a split as a plan file |
| `src/engine/handoff.rs` | The handoff document and the promote pull request |
| `src/engine/api.rs` | The local API |
| `src/engine/board.rs` | What a running scheduler is doing, written where `serve` can read it |
| `src/engine/editing.rs` | Checking a workflow draft, listing and scaffolding modules, proposing a change as a pull request |
| `app/` | The macOS app, a Swift package |
| `src/engine/lock.rs` | Locks that hold across processes |
| `src/engine/capacity.rs` | What the machine has, and how much to run at once |
| `src/engine/checkout.rs` | What `project add` checks in a checkout |
| `src/engine/config.rs` | This machine's settings: pools, the handoff folder |
| `src/engine/providers.rs` | This machine's subscriptions, and who may fill which role |
| `src/engine/qualify.rs` | The check a provider must pass before the engine uses it |
| `src/engine/ledger.rs` | Who is out of quota, and until when |
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
