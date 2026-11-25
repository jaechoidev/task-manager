# Findings: reading each plan's usage (2026-11-24)

The Schedule pane showed quota for GLM only. The other two were described as "known only from errors". That was never checked. Asked about it, the answer turned out to be that both say how much of the plan is used with every call.

## Result

| Provider | Where the reading comes from | When | Token handled by the engine |
| --- | --- | --- | --- |
| GLM | Z.ai's quota address | On request, at most once a minute | No. The shell passes the key to curl |
| Claude | An event in the streaming output of `claude -p` | With each call | No |
| ChatGPT | Codex's own session file | After each call | No |
| Antigravity | Not looked at. It fails its check anyway | | |

All three were read for real through the engine, with `factory providers check` on one tier each.

| Provider | Reading at 05:30 UTC on 2026-11-25 |
| --- | --- |
| ChatGPT | Weekly window 2% used. Plan `prolite` |
| Claude | 5-hour window 43% used, weekly window 37% used |
| GLM | 5-hour window 1% used, weekly window 1% used |

Tests: 122 unit tests and 55 scenarios. Two scenarios are new: a run leaves a reading of each plan, and a plan reported as used up sends the next call to someone else.

## How each is read

- **Claude.** In its streaming format Claude prints a `rate_limit_event` before the result. It holds a status (`allowed`, `allowed_warning`, `rejected`), the window it is about, and in this version every window of the plan with its share used and its reset time. The share is a fraction, which the Agent SDK's documentation says and the first probe did not: it read 0.36 for both windows. The engine used to ask for the single result, which leaves the event out. It now asks for the stream and takes the last result line.
- **Codex.** Its `exec --json` output has no such record. Its session file has: `sessions/<year>/<month>/<day>/rollout-<time>-<session>.jsonl` under its home folder gets a `rate_limits` record after each reply, with the percent used, the window in minutes, the reset time and the plan type. The engine knows the session id from the call's output, finds the file by its name and reads the newest record.
- **Where it is kept.** In `availability.json` beside Z.ai's reading, by provider. `factory providers`, the API and the app's pane already showed whatever is there, so they needed no new code to show three.

## What a reading does

- **A used-up plan sets the provider aside before a call fails.** For Codex that is a window at 100%, or Codex saying a limit was reached. For Claude it is a `rejected` 5-hour or weekly window.
- **For an hour at most.** Z.ai's reading can be asked for again at any time. These two can only be renewed by a call. A wrong reading with a reset a week away would keep a provider out for a week with nothing to correct it. So it holds for an hour, then one call is tried.
- **A window for one model does not put the provider out.** Claude has weekly windows for single models. A rejected one leaves the other models usable, and the failed call sets that model aside as before.
- **Claude's own word is the limit.** A failed call whose event says `rejected` is a usage limit, without looking for phrases in the error.

## Problems hit

1. **The first answer was a guess.** "Quota known only from errors" was written for two providers without looking. It was wrong for both.
2. **Reading the stream changed what counts as a limit sign.** The old check looked for limit phrases in every output line that mentions an error. In a stream that includes the agent's own messages, which may talk about rate limits. For a stream only the result line counts now.
3. **A stand-in did not know a fix round's prompt.** With the developer set aside, the fix round went to another provider, and the stand-in for it took the prompt for a review. The engine's prompt was fine: a fix round's prompt carries the whole issue. The stand-in now knows both openings.
4. **The check's own format changed.** Every Claude and GLM call now uses the stream. One tier of each was checked again for real and passes.

## Open items

- **Which login the Claude reading is for.** Every engine run so far was started from a shell inside a Claude Code session, where Claude takes that session's login. From an ordinary terminal it is the machine's own login. The reading is kept by provider name either way.
- **Codex's file is not a documented format.** An update can move or rename it. Then there is no reading, and limits are noticed from errors as before.
- **Readings go stale.** With no call there is no new number. The pane shows when each was read.
- **Only one tier of each was checked again.** The other tiers use the same call and were not rerun.
- **Antigravity.** Not looked at.
- **A real used-up plan.** Setting aside from a reading is tested with stand-ins only. Nobody was near a limit.
