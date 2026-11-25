# Findings: one scheduler for several projects (2026-11-24)

A machine can now work on several repositories with one set of places. It is built and tested with stand-ins. It has not run two real projects, because there is only one trial repository.

## Result

| | |
| --- | --- |
| Built | `factory project add, list, remove`. `factory schedule`, side by side or `--in-order`, with `--watch`. `factory queue` is now the same scheduler with one project |
| Tested with stand-ins | 114 unit tests and 53 scenarios. Five scenarios are new: side by side, in order, a project's own limit, watch mode, and an issue held by another process |
| Run for real | The commands against GitHub with nothing labelled: both registered repositories were listed and the scheduler ended cleanly |
| Not run for real | Two real projects with work in both. The machine guard holding an issue back. `--watch` for longer than a test |

## How it works

- **What was already shared.** Simulator slots, calls per provider and the quota ledger are files under the factory home, so two projects shared them before any of this. One scenario shows it: with one simulator slot, verifies of two projects still took turns.
- **What is new** is one count of places over all projects and a rule for the next free one.
  - Side by side: the project with the fewest issues running. Two places and two projects gave one issue of each, not two of the first.
  - In order: the first registered project with anything running or ready. The second started only after both issues of the first had ended.
- **A project can be capped.** With three places and a limit of one on the second project, its second issue waited for its first.
- **Watch mode** started with nothing labelled, picked up an issue labelled later, merged it, and was still running afterwards.
- **The guard** asks the same question as `factory capacity`: is there room for another simulator step this minute. It only applies to a further issue, so a busy machine still gets one.

## Problems hit

1. **The stand-in agents shared a call counter without a lock.** Two started in the same instant took the same number, and one read the other's prompt. Until now a lock per repository had staggered them. Two projects have two locks, so it showed at once: an issue of one project wrote the other's files and was stopped by the scope gate. The stand-ins now count under a lock. This is probably also the rare failure of the three-issue pool scenario seen earlier, which could not be explained then. That is a guess, not a finding.
2. **The first real three-issue run reported a merged issue as blocked,** because a second process for it was refused. The scheduler now tells that apart: such an issue is left alone.

## Open items

- **A real run with two projects.** It needs a second repository with its own plan.
- **Fair share counts issues, not cost.** A project whose issues take ten times longer gets the same number of places.
- **No service.** `--watch` is the daemon in the sense of the design doc, started by hand. A launchd entry is not written.
- **Priority between projects** is only the order they were added in. There is no weight.
- **The last of N issues is still verified N times** within a project. Unchanged here.
