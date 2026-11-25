# Findings: three real issues at once, with pools and providers (2026-11-24)

Pools and providers had only run against stand-ins. This is the first real run with both: plan `todo-v3`, three independent issues on the trial app, on one MacBook with three pool simulators and a providers file.

## Result

All three merged, with no fix round, no usage limit and nobody asked to look. Three simulator steps ran at the same time, each on its own simulator. The roles went where the providers file said.

| Issue | What | Time to merge | Verifies | Pull request |
| --- | --- | --- | --- | --- |
| #38 `settings-reset` | A button that resets the settings | 473 s | 1 | todo-trial#40 |
| #39 `row-done-badge` | A Done badge on a done todo's row | 780 s | 2 | todo-trial#42 |
| #37 `stats-open` | Stats tab counts the open todos | 1188 s | 3 | todo-trial#41 |

From the first start to the last merge: just under 20 minutes. Times are from the run logs, which started at 00:33 UTC on 2026-11-25.

## What was checked afterwards

- **`dev` is what was verified.** The tree of `dev` after the last merge is identical to the tree of the last commit that passed verify and review (pull request 41's head). Same hash.
- **Three at once is from the records, not the plan.** The step log gives each simulator step's end time and duration. Three overlapped for 234 seconds. The three first verifies ran on simulators 1, 2 and 3.
- **Who answered is from each call's own output.** The summaries name `glm-5.3-flash` as the model used, the reviews `claude-opus-5-5`.

| Job | Who the file lists first | Who answered | Calls |
| --- | --- | --- | --- |
| acceptance | No list, so the workflow's `claude` | Claude, default model | 3 |
| develop | `chatgpt sol` | ChatGPT, `gpt-6.2-sol` | 3 |
| review | `claude high` | Claude, `opus` | 6 |
| summary | `glm fast` | GLM, `haiku` mapped to `glm-5.3-flash` | 6 |

Nobody fell back, because nobody ran out. Fallback and parking are still untested against a real limit.

## Timing

| | Seconds |
| --- | --- |
| One verify alone, measured earlier the same day | 197 |
| The three first verifies, all overlapping | 231, 236, 243 |
| Later verifies, two overlapping | 225, 234 |
| The last verify, alone but after the others | 257 |

Three at once cost 17 to 23% more per verify here. Earlier the same day it was 12% once and 41 to 46% once. So the spread on this machine is wide, and a pool of 3 was worth it each time.

## What the run showed

- **The merge turn did its job.** #37 printed "waiting for another issue's merge to finish", then found `dev` moved and verified again. Without the turn, that is the moment two issues merged unverified.
- **The last issue is verified once per issue ahead of it.** Each merge moves `dev`, and whoever is behind takes it in and is verified, reviewed and summarized again. Three issues needed 6 verifies, 6 reviews and 6 summaries. With N issues at once the last one runs N times. That is what "never merge unverified" costs when merges are one at a time. Verifying a batch together would cost less, and is not built.
- **GLM's fast tier was not slow here.** 13 to 36 seconds per summary. It had taken minutes in the check.

## Problems hit

1. **The queue started two issues, not three.** The `today` label went on three seconds before the queue started, and GitHub's list of labelled issues did not have #39 yet. The queue only looked again when a run ended. Fixed: with a place free it now looks again every 30 seconds, so an issue labelled while the queue runs is picked up too. There is a scenario for it.
2. **#39 was started by hand, so the queue's summary is wrong about it.** To get three at once, `factory run 39` was started directly. When the queue later tried #39, the second process was refused, as it should be, and the queue counted that as a failure. Its last line says "Blocked: #39" and it exited 1. #39 is merged and closed, and `factory blocked` lists nobody.

## Open items

- **A refused second process should not count as blocked.** The queue could tell "already being run" from a failure.
- **Fallback and parking against a real usage limit.** Still only tested with stand-ins.
- **The ChatGPT tier names** in the providers file are placeholders (`sol`, `astra`).
- **Not yet on the Mac Studio.** Everything here ran on the MacBook, as `jaechoidev`.
