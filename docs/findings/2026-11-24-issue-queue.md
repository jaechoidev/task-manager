# Findings: the issue queue (2026-11-24)

Build-order step 6: the issue queue from the dependency graph, ready issues by priority, parallel development where scopes don't overlap, resume after a crash, and waiting out rate limits.

## Result

`factory queue` took a three-issue plan on `jaechoidev/todo-trial` from nothing to three merged pull requests in 18 minutes 30 seconds, with nobody involved. `dev` passes all eight flows afterwards.

The plan, [todo-trial#21](https://github.com/jaechoidev/todo-trial/pull/21), had one issue first and two that could run together:

| Issue | Pull request | Time | What happened |
| --- | --- | --- | --- |
| #24 tabs, alone | [#28](https://github.com/jaechoidev/todo-trial/pull/28), merged | 6 min 6 s | A clean pass. The acceptance writer added three IDs for the tab bar buttons |
| #25 stats, high priority | [#29](https://github.com/jaechoidev/todo-trial/pull/29), merged | 6 min 21 s | A clean pass, in parallel with #26 |
| #26 settings | [#30](https://github.com/jaechoidev/todo-trial/pull/30), merged | 12 min 20 s | Passed, then found that #25 had merged first, took `dev` in, and was verified and reviewed a second time |

When each step of the two parallel issues ran, from the request log:

| Step | #25 | #26 |
| --- | --- | --- |
| acceptance | 20:37:30 to 20:38:05 | 20:37:31 to 20:38:11 |
| red, needs the simulator | 20:38:05 to 20:38:53 | 20:38:52 to 20:39:39 |
| develop | 20:38:52 to 20:39:21 | 20:39:39 to 20:40:05 |
| verify, needs the simulator | 20:39:39 to 20:42:24 | 20:42:23 to 20:44:53 |
| verify again, after `dev` moved | | 20:45:10 to 20:48:16 |

The two acceptance writers worked at the same time. Every step that needs the simulator waited its turn: #26 logged "waiting for the simulator lock" three times, and no two simulator steps overlap.

| Part | What it does | How it was checked |
| --- | --- | --- |
| Queue | Runs every ready issue labelled `today`, by priority, up to `--parallel` at once | 4 engine scenarios with several issues and real merges into a stand-in origin, plus unit tests |
| Parallel work | One process per issue. Development overlaps in time. Verify takes the simulator lock | A scenario that records when each developer and each verify started and ended |
| Resume | A run cut off mid-way picks up at the step it was on | Scenarios that kill the engine at verify and while waiting on GitHub |
| Usage limits | A limited call is waited out and made again | A scenario with a stand-in Codex that reports a usage limit once |

91 tests passed at the end of this step: 68 unit tests and 23 engine scenarios.

What the trial showed that the stand-ins could not:

- **Parallel work saves less than it looks.** Two issues took 12 minutes together and would have taken about 12 and a half one after the other. On an iOS project almost all the time is the simulator, which only one issue can use. The agents' own calls are short.
- **The second issue to merge pays for it.** #26 was verified twice, three minutes each, because #25 merged first. With more issues in parallel, the last one could be verified several times.
- **GitHub closed each merged issue fast enough.** The queue saw #24 closed within seconds and started the two issues it blocked.

## What the scenarios showed

- **Two independent issues were developed at the same time.** Both developers had started before either finished, and no two verify runs were ever inside the simulator lock together.
- **The second issue to merge was judged again.** It found that the base branch had moved, merged it in, and ran verify and review once more before its own merge. The moved-base check from the GitHub loop is what makes parallel merging safe.
- **An issue blocked by another started only after that one merged,** and was cut from a base that already held its work.
- **Issues whose scopes overlap ran one after the other,** although nothing ordered them.
- **A run cut off while waiting on GitHub did not open a second pull request,** and did not call the developer or the reviewer again.

## Where the build differs from the design doc

- **The queue is a command, not a daemon.** `factory queue` runs until nothing is ready and then exits. The doc's engine is one long-running process under launchd with a local API. That belongs with the app.
- **One process per issue.** The doc does not say how issues run in parallel. Separate processes keep a crash in one from touching the others and let the operating system clean up locks.
- **Scope overlap is checked again at run time.** The plan validator already refuses unordered issues with overlapping scopes. The queue checks anyway, because an issue's scope can be edited after the plan was validated, which is what happened to four issues of the first trial plan.
- **Resume repeats the interrupted step.** The doc says the engine resumes from the database. Here it resumes from a file written before each step, and whatever that step had half done is thrown away.
- **Usage limits are waited out on a fixed schedule,** not until the limit's reset time. Neither CLI gives the reset time in a form the engine can rely on.

## Problems hit

1. **The plan PR could not be merged by Claude.** The earlier approval to merge passing plan PRs on the trial repo was refused by the permission system this time. Jae merged it, and the trial ran after that.
2. **A lock test failed once in a full run.** Another test was starting a child process at the same instant, and for a moment that child held a copy of the lock file. Only tests start processes from several threads at once. The test now allows for it.

## Open items

- **Priority order has only been seen with stand-ins.** In the trial the two parallel issues started together, so the high-priority label decided nothing.
- **Resume has not been needed on GitHub.** No run was cut off during the trial.
- **Usage-limit detection has never met a real limit.** The phrases are taken from each CLI's documentation and source.
- **A queue hit by a usage limit keeps starting issues.** Each run waits on its own. Nothing tells the queue that a vendor is limited.
- **Two developers at once means two Codex sessions at once.** The effect on usage limits is unmeasured.
- **An issue that was closed while its run was cut off stays "running" in `factory status`.**
- **Nothing closes a plan's parent issue** when its sub-issues are done.
