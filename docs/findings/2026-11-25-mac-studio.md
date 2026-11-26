# Findings: the Mac Studio's first issues, as neostudiodot (2026-11-25)

Every run before this was on the MacBook as `jaechoidev`. This sets `factory` up on the Mac Studio, the machine meant to run issues from now on, and runs one real issue there as the agent account `neostudiodot`, with `factory schedule` started by hand. The service was installed afterwards, and a second issue ran under it the next day, with no Claude Code session involved.

## The first issue, started by hand

[counter-trial#17](https://github.com/jaechoidev/counter-trial/issues/17) `counter-add-ten` merged as [counter-trial#18](https://github.com/jaechoidev/counter-trial/pull/18) in 300 seconds, on the first attempt, with no fix round and nobody asked to look. The plan pull request [counter-trial#15](https://github.com/jaechoidev/counter-trial/pull/15) was opened as `neostudiodot` and approved and merged by Jae.

| Step | Who answered | Result | Seconds |
| --- | --- | --- | --- |
| acceptance | Claude, `claude-fable-5-2` | Wrote `counter_add_ten.yaml` | 13 |
| red | engine, `verify.sh --red` | The new flow fails on the app as it was | 70 |
| develop | ChatGPT, started with `gpt-6.2-sol` | One file changed | 27 |
| gates | engine | locked flows, scope, IDs: pass | 0 |
| verify | engine, `verify.sh`, 7 flows | Pass | 102 |
| review | Claude, `claude-opus-5-5` | approve, on the same commit | 8 |
| summary | Claude, `claude-haiku-5-5` | Written | 10 |
| pr, github, merge | engine | Pushed, review check green, squash merged | 4, 49, 4 |

The model names are from each call's own output, except the developer's: Codex's output does not name its model, so that is what the engine asked for.

### What was checked afterwards

- **`dev` is what was verified.** The tree of `dev` after the merge, `6041eac`, is the tree of `69e0d1e`, the commit that passed verify and review.
- **The agent account did the GitHub work.** The pull request's author and the account that merged it are both `neostudiodot`. The issue closed as completed 2 seconds after the merge.
- **The GitHub review counted.** Its comment is from `github-actions[bot]`, verdict approve, for `69e0d1e`.
- **Nothing waits.** `factory blocked` lists no issue.

## The second issue, run by the service (2026-11-26)

After the first run, the service was installed: both launchd agents run from `~/.cargo/bin/factory`, built at `655291c`, and the API answers on `127.0.0.1:7878`. The plan, [counter-trial#19](https://github.com/jaechoidev/counter-trial/pull/19), was opened from the MacBook as `jaechoidev` and merged by Jae. The repository's plan-create workflow made the issue, and it was labelled `today` from the MacBook. Nothing on the MacBook was watching the trial repos, so only the Mac Studio could take it.

[counter-trial#21](https://github.com/jaechoidev/counter-trial/issues/21) `about-tagline` merged as [counter-trial#22](https://github.com/jaechoidev/counter-trial/pull/22) in 289 seconds, on the first attempt, with no fix round and nobody asked to look.

| Step | Who answered | Result | Seconds |
| --- | --- | --- | --- |
| pick up | service, `factory schedule --watch` | Started after the `today` label | 32 |
| acceptance | Claude, `claude-fable-5-2` | Wrote `about_tagline.yaml` | 14 |
| red | engine, `verify.sh --red` | The new flow fails on the app as it was | 50 |
| develop | ChatGPT, started with `gpt-6.2-sol` | One file changed, inside scope | 26 |
| verify | engine, `verify.sh`, 8 flows | Pass | 116 |
| review | Claude, `claude-opus-5-5` | approve, on the same commit | 9 |
| summary | Claude, `claude-haiku-5-5` | Written | 7 |
| github, merge | engine | Review check green, squash merged | 49 |

### What was checked afterwards

- **The machine's own logins answered.** The service's environment holds only `PATH`, so every Claude and Codex call used this machine's own login, not a Claude Code session's. That was the part the first run could not show.
- **`dev` is what was verified.** The tree of `dev` after the merge, `34ebe86`, is the tree of `b9e0c4d`, the commit that passed verify and review.
- **Every commit carries the agent's name.** Both commits on the branch, `ba576b5` and `b9e0c4d`, are authored and committed by `neostudiodot`, and so is the squash `34ebe86`. This is the first run since the git identity change in the open items below.
- **The agent account did the GitHub work.** The pull request's author and the account that merged it are both `neostudiodot`. The issue closed as completed 1 second after the merge.
- **The GitHub review counted.** Its comment is from `github-actions[bot]`, verdict approve, for `b9e0c4d`.
- **The watcher outlasted GitHub's errors.** The scheduler had been up for about 17 hours when the label went on. In that time it logged two 503 answers and one cut-off reply while listing issues, said so, and kept looking.
- **Nothing waits.** `factory blocked` lists no issue.

## The machine

| | |
| --- | --- |
| Hardware | Apple M1 Max, 10 cores (8 performance), 64 GB |
| Software | macOS 26.7.3, Xcode 27.2, iOS 27.2 simulator runtime, Command Line Tools 26.6 |
| Proposal | 2 simulator steps. The cores limit it: memory would allow 7 |
| Pools written | simulator 2, chatgpt 2, claude 2 |
| `capacity --prepare` | `factory-sim-1` and `factory-sim-2`, iPhone 17 on iOS 27.2, in 5 minutes 8 seconds |
| Build | `cargo build --release` in 29 s. `cargo test`: 135 unit tests and 55 scenarios pass |
| Provider checks | chatgpt `sol`, `astra`, `configured` and claude `high`, `standard`, `fast` all qualified |

## What was different from the MacBook

- **Nothing a run needs was installed.** No Homebrew, Rust, Java or Maestro. Homebrew's installer asks for a password, so it is a person's step. Rust 1.100 came from Homebrew's `rustup`, and Maestro 2.12.1 from its tap, which pulls in `openjdk` 27 as its own dependency.
- **Homebrew refuses an untrusted tap.** `brew install mobile-dev-inc/tap/maestro`, as the README has it, fails until `brew trust --formula mobile-dev-inc/tap/maestro` is run.
- **Java and Rust are not on PATH after install.** Both formulae are keg-only. `/usr/bin/java` is Apple's stub and fails without a JDK it knows, and Maestro's start script uses the `java` it finds on PATH. A line in `~/.zprofile`, added by hand, puts `openjdk/bin`, `rustup/bin` and `~/.cargo/bin` first. The service takes its PATH from the shell that installs it, so that line is what lets the service find them. The service's run found both.
- **Preflight needs the owner.** Its first call reads `repos/<repo>/invitations`, which needs admin. As `neostudiodot`, which has write, it fails with 403 and exits 2. Run from the MacBook as `jaechoidev`, all 16 checks pass on both trial repos. An agent account can't preflight itself.
- **Who pushes is git's credential, not `gh`'s.** The engine runs plain `git push`. Here the keychain's `github.com` entry, used through Xcode's `osxkeychain` helper, is `neostudiodot`. `gh` is also logged in as `jaechoidev`, not active. If the keychain held the other account, pushes and `gh api` calls would come from different accounts.
- **The squash commit is the agent's.** The engine's own commits carry this machine's git identity, Jae's. The squash commit on `dev` is authored by `neostudiodot`'s GitHub identity, because the account that squash merges authors the result. On the MacBook both were Jae.
- **GLM is left out.** There is no `~/.zai/env.sh` here. So each job has one name: develop ChatGPT `sol`, review Claude `high`, summary Claude `fast`. The summary went to Claude, where the MacBook's went to GLM. If ChatGPT runs out, an issue parks, because nobody else may develop.
- **The acceptance writer ran on Fable.** Acceptance has no list in `providers.yaml`, so it falls back to the workflow's `claude` with no model, which is the program's default. `~/.claude/settings.json` on this machine names `fable`. That this setting is where the default came from is a guess, not checked. The MacBook's findings say "default model" without naming it.
- **Verify took 102 seconds** for 7 flows, and 116 seconds for 8 under the service. On the MacBook, counter-trial's verify took 91 to 107 seconds with five or six flows ([a view of the scheduler](2026-11-24-schedule-view.md)), so this machine is in line with it.

## Not covered yet

- **Two issues at once on this machine,** and todo-trial, whose verify is about 300 seconds.
- **GLM, a restart, and a usage limit.**

## Open items

Three were found here and are done on `main`:

- **Preflight as the agent account.** `repo init` and `repo preflight` now stop early with a clear message, exit 2, when `gh`'s account isn't an admin. The README says to run preflight as the owner. `ab2f8bc`.
- **The README's Maestro line.** It now runs `brew trust --formula mobile-dev-inc/tap/maestro` first, and gives the `~/.zprofile` line for `openjdk`, `rustup` and `~/.cargo/bin`. `0afc3b0`.
- **The acceptance writer's model.** The engine already read `roles.acceptance`. Only the files lacked it. `factory providers` now names each job with no list as not pinned to a model, the example file gives acceptance a list, and the app says so for an empty list. `a6bf483`. Pinning it on the Mac Studio is still an edit of its `providers.yaml`, and Jae's choice. With develop on ChatGPT only, the acceptance writer has to come from another family, so Claude at some tier.

One more is done on the Mac Studio itself, not in the code:

- **Whose name the engine's commits carry.** The engine makes its worktrees from `~/code/counter-trial` and `~/code/todo-trial`, and a worktree uses its checkout's config. Both checkouts now set `user.name` to `neostudiodot` and `user.email` to its GitHub no-reply address, so the engine's commits are the agent's. The global `~/.gitconfig` is still Jae's, so commits made by hand in other repos stay Jae's. The two commits on counter-trial#18's branch were made before this and keep Jae's name. Its squash on `dev`, `6e3441a`, was made by GitHub when `neostudiodot` merged it, so it already carries `neostudiodot`'s name. Every commit of counter-trial#21, the first issue after the change, is `neostudiodot`'s. The engine doesn't set an identity itself, so each new agent machine needs this step for each checkout.
