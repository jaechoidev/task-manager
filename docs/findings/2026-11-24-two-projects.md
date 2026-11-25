# Findings: two real projects under one scheduler (2026-11-24)

The scheduler had only run against stand-ins, because there was one trial repository. This is the first real run of `factory schedule`: two repositories, two issues each, three places, on one MacBook.

## Result

All four merged, with no fix round, no usage limit and nobody asked to look. The places went to the projects as the rule says. Simulators and provider calls were shared across both projects.

| Issue | What | Time to merge | Verifies | Pull request |
| --- | --- | --- | --- | --- |
| counter-trial#3 `counter` | Count up and down | 360 s | 1 | counter-trial#5 |
| counter-trial#4 `about` | App name and version | 246 s | 1 | counter-trial#6 |
| todo-trial#46 `settings-build` | Build number in Settings | 544 s | 1 | todo-trial#47 |
| todo-trial#45 `stats-percent` | Share of todos done | 1007 s | 2 | todo-trial#48 |

From the first start to the last merge: 16 minutes 45 seconds. The run started at 04:11 UTC on 2026-11-25.

## The second project

`jaechoidev/counter-trial` was created for this with `factory repo init` from the template: private, the same branches, rulesets and labels. Its app is a counter with two tabs. Its preflight passed all 16 checks once the agent account had accepted its invitation and the review token was set. Both of those are a person's steps.

## What was checked afterwards

- **Each `dev` is what was verified.** In both repositories the tree of `dev` after the last merge is identical to the tree of the last commit that passed verify and review. Same hash.
- **The places followed the rule.** With three places and nothing running, side by side gave them to todo-trial, counter-trial, todo-trial: fewest running first, the first registered project on a tie. When counter-trial#3 merged, the free place went to counter-trial#4 nine seconds later.
- **Simulators were shared across projects.** Three simulator steps at once, on simulators 1, 2 and 3, with two projects among them, three times in the run. Times are each step's end time minus its duration.

| When (UTC) | What overlapped | For |
| --- | --- | --- |
| 04:12:01 to 04:13:18 | The failing-first check of todo-trial#46, counter-trial#3, todo-trial#45 | 77 s |
| 04:14:06 to 04:14:42 | The verify of the same three | 36 s |
| 04:17:51 to 04:18:27 | counter-trial#4's failing-first check and both todo-trial verifies | 36 s |

- **Provider calls were shared too.** With a pool of 2 per provider, the third acceptance call printed "waiting for a free claude call, all 2 in use", and the third developer the same for ChatGPT. The first two of each belonged to different projects.
- **Who answered is from each call's own output,** except the developer.

| Job | Who the file lists first | Who answered | Calls |
| --- | --- | --- | --- |
| acceptance | No list, so the workflow's `claude` | Claude, default model | 4 |
| develop | `chatgpt sol` | ChatGPT. The engine started it with `gpt-6.2-sol`. Codex's output does not name its model | 4 |
| review | `claude high` | Claude, `claude-opus-5-5` | 5 |
| summary | `glm fast` | GLM, `glm-5.3-flash` | 5 |

- **GitHub's review check passed on the new repository,** so the token set by hand works there.

## Timing

| | Seconds |
| --- | --- |
| counter-trial verify, 3 flows | 46, 55 |
| todo-trial verify, 13 flows, two overlapping | 284, 313 |
| todo-trial verify, alone, last | 299 |
| todo-trial verify alone, measured in the morning with fewer flows | 197 |

A todo-trial verify is now about 300 seconds even alone. The likely cause is that its flow suite grows with every merged issue. That was not measured, so it is a guess.

## What the run showed

- **A small project finishes inside a large one's verify.** counter-trial ran both its issues, one after the other, in the time todo-trial took for its first verifies. Fair share by issue count gave counter-trial one place of three, and that was enough here.
- **The cost of one merge at a time is per project.** todo-trial#45 was verified twice, because todo-trial#46 merged first. counter-trial#4 was verified once: its first issue had merged before it started. Two projects don't make each other verify again.
- **The developer never ran the app.** Both counter-trial handoff notes say the developer's sandbox blocked the simulator, so the engine's verify is the only execution the change had. Known, and the reason verify is a gate.

## Not covered by this run

- **`--in-order`, `--watch` and a limit per project.** Only side by side ran for real.
- **The machine guard holding an issue back.** It never had to.
- **Fallback and parking after a real usage limit.** Nobody ran out.
- **The Mac Studio and the agent account.** Everything ran on the MacBook, as `jaechoidev`.

## Open items

- **No view of the scheduler.** The macOS app lists issues with their steps, and labels each with its repository. It does not show the places, what waits for one, which simulator a step holds, who answered a call, or what is parked until when. Those are in the logs only. The app's bundle also predates pools, providers and the scheduler, and nobody has used it with two projects.
- **`factory status` is one long list.** It prints every issue the machine has run, with no filter by project or by run.
- **Verify time grows with the suite.** At 300 seconds for 13 flows, an issue that has to be verified again costs five minutes each time. Verifying a batch together, or running only the flows a change can affect, is not built.
- **Fair share counts issues, not cost.** A todo-trial issue held a place two to four times as long as a counter-trial one.
