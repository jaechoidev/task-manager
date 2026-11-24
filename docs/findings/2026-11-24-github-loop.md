# Findings: the GitHub loop (2026-11-24)

Build-order step 5: poll CI and the GitHub review on the head commit, fix rounds, merge to `dev`, and re-verify when `dev` moved. Tried on `jaechoidev/todo-trial` issue #7, and in automated tests that run the real engine against a stand-in GitHub.

## Result

`factory run 7` took the issue from nothing to a merged pull request, [todo-trial#19](https://github.com/jaechoidev/todo-trial/pull/19), in 4 minutes 58 seconds, with no person involved.

| Step | Who | Result | Time |
| --- | --- | --- | --- |
| acceptance | Claude | Wrote `.maestro/filter_todos.yaml` | 28 s |
| red | `./verify.sh --red` | The new flow fails on the old app | 56 s |
| develop | Codex | Built the filter. The ID gate held it back: the IDs were built by string interpolation | 32 s |
| develop, fix round 1 | Codex, same session | Wrote the three IDs as literals | 17 s |
| verify | `./verify.sh` | 4 of 4 flows passed | 77 s |
| review | Claude | approve | 12 s |
| sync | engine | `dev` had not moved | 1 s |
| pr | engine | Pushed, opened #19 | 3 s |
| github | engine, waiting | The `review` check finished and its verdict for this commit was approve | 66 s |
| merge | engine | Squash-merged at the judged commit, deleted the branch and the worktree | 4 s |

This was also the first fix round on the iOS project with real agents, and it came from a gate, not from a flow.

A second run, on issue #8, confirmed it: [todo-trial#20](https://github.com/jaechoidev/todo-trial/pull/20) was merged in 4 minutes 55 seconds with no fix round, and its merge commit carries the developer's subject. With it, every issue of the trial plan is closed and `dev` passes all five flows.

The automated tests cover what the real run did not: a GitHub review asking for changes, a red check, GitHub rounds running out, a check that passed with no verdict, a verdict for a different commit, an approval typed by someone other than the workflow, a base branch that moved, `--no-merge`, and a repository with no `github` block. 15 scenarios in `tests/fix_loop.rs`, 75 tests in total.

## A green check is not an approval, and now the engine knows

The earlier finding was that the `review` check went green whenever the review ran, whatever it concluded. Two changes close that:

- **The workflow posts a machine-readable verdict.** `review.yml` asks the model for a structured answer and a plain script posts it. The comment's first line is `<!-- factory:review {"verdict":...,"sha":...} -->`. The commit comes from the event, not from the model. The check is now red when the verdict is "changes".
- **The engine trusts only that line.** It must come from `github-actions[bot]`, which only a workflow in the repository can post as, and its commit must be the pull request's head. A green check with no such line stops the run, because the usual cause is a missing token secret.

Checked on GitHub before the engine relied on it: [todo-trial#18](https://github.com/jaechoidev/todo-trial/pull/18) carried the new workflow and reviewed itself.

## Answers to the design doc's open questions

| Question | Answer |
| --- | --- |
| When should GitHub review be a third model | Not answered. Both reviews are Claude today, because the only CI secret is a Claude token. The second review adds a fresh session and a different prompt, not a different model |

## Where the build differs from the design doc

- **Merge, not rebase, when `dev` moved.** The doc says to rebase onto `dev`. A rebase of a pushed branch needs a force push, and the doc's own rules forbid one. The engine merges `dev` into the issue's branch. The pull request is squash-merged in the end, so `dev`'s history is the same either way.
- **A conflict stops the run.** The developer is not asked to resolve it. Its scope gate compares against the last commit, and a merge in progress would look like a change to every file that came in from `dev`.
- **The GitHub steps are the engine's, not modules.** `sync`, `pr`, `github` and `merge` are not in `factory.yaml`. A `github` block turns the last two on.
- **GitHub rounds default to 2,** as the doc suggests, with their own budget. Local rounds spent after a GitHub round still count as local.
- **Checks are judged by name and conclusion only.** The review check is special because its verdict is read from the comment. Any other failed check goes back to the developer with the end of its log.

## Problems hit

1. **A run whose first attempt was held by a gate got the wrong names.** The pull request for #7 was titled after the fix round ("fix(filter): use literal accessibility identifiers") and the merge commit after the issue. The first commit now carries the subject of the first attempt. Found on the real run, fixed, and covered by a test.
2. **The ID gate cost a round for code that worked.** Codex set `"todo.filter.\(name)"`, which gives the right IDs at run time. The gate wants each ID as a literal, so it sent the change back. The fix took 17 seconds. The gate is stricter than the flows need, and it is also the only check on an ID no flow uses.
3. **Issue #7 needed its scope widened by hand,** like #5 and #6.

## Open items

- **A run that is interrupted while waiting on GitHub can't be picked up.** `factory run` refuses an issue whose branch is already pushed. Resuming belongs to the queue step.
- **The merge is not atomic with the last sync check.** `dev` could move between the engine's check and GitHub's merge. GitHub would still merge if there is no conflict, and that result would not have been verified. One issue at a time makes this unlikely today.
- **No build runs on GitHub.** The trial repo's only check is the review. An iOS build needs a macOS runner.
- **`factory status` is still wrong for issues merged by hand before this step,** such as #6.
- **Not run on the Mac Studio or as `neostudiodot`.** The merge used this machine's `gh` login.
- **A workflow can be edited by the pull request it reviews.** The developer's scope gate keeps the engine's own agents out of `.github/`. A person with write access is not stopped.
