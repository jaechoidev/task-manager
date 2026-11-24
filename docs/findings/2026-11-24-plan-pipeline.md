# Findings: bootstrap and plan pipeline (2026-11-24)

The goal was met: a project created from the template and the script, where a merged plan file became a correct issue graph.

| Repo | Role |
| --- | --- |
| [jaechoidev/task-manager](https://github.com/jaechoidev/task-manager) | The `factory` crate. Public, so project CI can build it without a secret |
| [jaechoidev/app-template](https://github.com/jaechoidev/app-template) | Template repo. Private |
| [jaechoidev/todo-trial](https://github.com/jaechoidev/todo-trial) | Trial project. Private |

## Results against the plan

| Step | Result |
| --- | --- |
| `factory` crate with `repo init`, `plan validate`, `plan create` | Done. 17 unit tests pass. `repo preflight` added as its own command |
| Template repo with both plan workflows | Done |
| Trial project from `factory repo init`, preflight passes | Done: 14 passed, 2 warnings, 0 failed |
| Dummy plan: one parent, a chain, a parallel pair | Done: `todo-v1`, 5 child issues, 4 batches |
| Plan PR: check passes and comments the batch list | Done: [todo-trial#1](https://github.com/jaechoidev/todo-trial/pull/1) |
| Negative PR: broken plan fails and can't merge | Done: [todo-trial#2](https://github.com/jaechoidev/todo-trial/pull/2), blocked. Left open as evidence |
| Issues exist with labels and body fields | Done: #3 (parent) and #4 to #8 |
| Parent and blocked-by links match the plan | Done: 5 sub-issue links, 4 blocked-by links, read back through the API |
| Merged PR lists the issue numbers | Done |
| Rerunning `plan create` creates nothing new | Done, locally and in CI: 0 created, 6 already existed, 0 links added |

## Findings the plan asked for

**Token permissions.** The default `GITHUB_TOKEN` with `issues: write` can create issues, add sub-issues and add blocked-by dependencies. No personal token or bot secret is needed. Commenting on the pull request also needs `pull-requests: write`.

**`gh` version.** The `ubuntu-24.04` runner image (20261116.376.1) ships `gh` 2.104.0. This Mac has 2.79.0, older than the 2.94.0 the design names for `--blocked-by`. `factory` avoids the question: it calls the REST endpoints through `gh api`, which works on both.

**Run times.**

| Run | Time | Notes |
| --- | --- | --- |
| `plan-validate`, cold | 50 s | Includes compiling `factory` on the runner |
| `plan-create`, first run | 84 s | Cold compile, then 15 writes paced 1 s apart |
| `plan-create`, rerun | 17 s | Cached binary, reads only |
| `plan create`, local rerun | 3.6 s | |
| `plan validate`, local | under 0.1 s | |

## Where the build differs from the design doc

- **Links use `gh api`, not `gh issue create --parent` / `--blocked-by`.** Same result, no dependency on the `gh` version, and the response carries the issue ID the link endpoints need.
- **`plan-validate` runs on every pull request into `main`, with no path filter.** The check is required. With a path filter, a pull request that doesn't touch `plans/` would wait on it forever, which would block dev promotions.
- **`main` has two rulesets, not one.** The review rule (1 approval, stale approvals dismissed, last push approved) lets the owner bypass it on a pull request, so the owner can merge their own plan. The guard rule (no force push, no deletion, `plan-validate` required) has no bypass at all.
- **The validate comment is updated in place** on each push, found by a marker line, so a pull request has one validation comment.
- **`plan-create` also has a manual trigger** that processes every plan, to repair a run that was skipped or failed.
- **Issue bodies start with one JSON line in an HTML comment.** This is the contract the engine will read. The markdown below it is for people.
- **Plan files gained fields the doc implied:** a plan `title` for the parent issue, `accessibility_ids`, and `{file, new: true}` for flows.
- **Labels:** `in-progress` was added for the state mirror. `priority:*` became `priority:high`, `priority:normal`, `priority:low`.

## Problems hit

1. **A repo generated from a template is empty for a few seconds.** GitHub answers 409 until the copy lands. `repo init` now waits.
2. **`runner.temp` can't be read in job-level `env`.** Both workflows were invalid, and an invalid workflow fails on every push to any branch. Fixed in the template. On the trial repo the fix rode in with the plan pull request.
3. **`dev` on the trial repo was cut from `main` before the fix,** so it still held the invalid workflow files. Fixed the same day by fast-forwarding `dev` to `main`. The push to `dev` started no runs.

## Open items

- **`main` and `dev` drift.** Plans and workflow fixes land on `main`. Nothing brings them to `dev`. The design needs a step for this.
- **Factory is not pinned in CI.** Workflows build the head of `task-manager` `main`. A release tag would make runs reproducible.
- **Queued `plan-create` runs.** A concurrency group keeps one waiting run. A third merge in quick succession may replace the second one's run. The manual trigger repairs it. Not tested with real concurrent merges.
- **Cross-plan scope overlap is not checked.** References to other plans (`other-plan/key`, `"#42"`) work. Comparing a new plan's scopes with other plans' open issues needs GitHub state and is not built.
- **Editing a merged plan does not change its issues.** Existing issues are never edited.
- **`review.yml` is untested**, and `CLAUDE_CODE_OAUTH_TOKEN` is not set on the trial repo.
- **neostudiodot has not accepted the invitation** to the trial repo.
- **"Only the owner can merge into main" is not applied.** The pilot repo has that rule. Here neostudiodot could merge a pull request into `main` once it has an approval.
- **The plan-to-issues skill is not written.** Today covers the deterministic half: validate and create. The part where an LLM drafts `plan.yaml` and fixes validation errors is still to do.
