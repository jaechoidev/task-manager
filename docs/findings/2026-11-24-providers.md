# Findings: providers, roles with fallback, and parking (2026-11-24)

The engine knew two vendors, one login each. This adds what a machine actually has: several subscriptions, a model per tier, an order to try them in for each job, and a way to wait for quota without blocking.

## Result

Built and tested with stand-ins. The check ran for real on four subscriptions. The engine has not yet run a real issue with a providers file.

| | |
| --- | --- |
| Built | `providers.yaml`, `factory providers`, `factory providers check`, roles as ordered lists, independence by family, a shared quota ledger, parked issues, a queue that works around them |
| Tested with stand-ins | 112 unit tests and 47 scenarios. Seven scenarios are new |
| Run for real | The check, on 12 tiers of 4 subscriptions, twice in full as the check itself was corrected, after a first partial run. Z.ai's quota read. One credential test against a local listener |
| Not run for real | A real issue with a providers file. Fallback and parking after a real usage limit |

## What the check found

Every tier of every subscription, from a folder under `~/.factory`, outside any repository.

| Subscription | Runs through | Tiers tried | Verdict |
| --- | --- | --- | --- |
| Claude | `claude` | `haiku`, `sonnet`, `opus` | Qualified. Refused both the read and the write outside |
| GLM | `claude` with `~/.zai/env.sh` | `haiku`, `sonnet`, `opus`, which that file maps to `glm-5.3-flash` and `glm-5.3` | Qualified, confined like Claude |
| ChatGPT | `codex` | `gpt-6.1-sol`, `gpt-6.2-sol`, `gpt-6.2-astra` | Qualified. Reads outside its folder, which was known and accepted |
| Antigravity | `agy` | `gemini-3.8-flash-medium`, `gemini-4-hydrogen-high`, `claude-sonnet-4-6` | **Failed.** Wrote into the home directory |

## A credential went to the wrong service

This is the finding that matters most, and it happened during this work.

- **What happened.** The first GLM checks failed with a 401 from Z.ai. They ran from a shell inside a Claude Code session. In that shell `claude` ignores the key an env file sets and sends a bearer token the session supplies. So four calls (three check tiers and one by hand, each possibly retried) sent that Claude credential to `api.z.ai`, over TLS, where it was refused.
- **How it was established.** The same call pointed at a listener on `127.0.0.1`, with a dummy token in the env file. The listener saw a different bearer token, 115 characters with its prefix. Only the kind and length were recorded. From a clean environment the dummy token arrived, and the real call answered as `glm-5.3-flash`.
- **The fix.** A provider with its own `env_file` or `config_dir` is started without the inherited variables that could select another login: `ANTHROPIC_*`, `CLAUDE_*` and `CLAUDECODE` for `claude`, `OPENAI_*` and `CODEX_*` for `codex`. Verified through the engine from the same shell: the listener then saw the dummy token.
- **What it does not cover.** A provider on the program's default login still inherits everything, which is what every earlier run did. And the token that went out is not revoked by any of this. Logging in again replaces it.
- **Not affected.** `zai-code` in an ordinary terminal.

## Antigravity is not confined

- **Under `--sandbox` it wrote into a new, ordinary folder in the home directory.** Tried by hand three times, from a folder under `~/.factory`: into a repository, into the same place with its trusted-workspace list emptied for the call, and into a folder created seconds before.
- **It does refuse some places.** A write into a folder next to its own, under `~/.factory`, was refused. Hidden folders look protected. Ordinary ones are not. Whether it can read next to its folder was not settled, because its run ends at the first refusal.
- **The first version of the check passed it,** twice, for two different reasons, both now fixed:
  1. The only place it probed outside was next to the test folder, which Antigravity happens to refuse. The check now also tries a new folder in the home directory, and removes it.
  2. With both probes in one call it still passed. Antigravity ends a run at the first tool it is refused, so it never reached the second probe. Each probe now has a call of its own.
- **Where the check runs matters.** From a scratch home inside a repository, Antigravity wrote next to its folder too. The check runs under the factory home because that is where a real run's worktree is.
- **Consequence.** Antigravity fails its check, and the engine only uses what passed. It stays in the file as a provider nobody can use.

## How it works

- **A role is a list.** Who answers a call is the first entry that passed its check, is independent, and is in quota. A role with no list uses the workflow file's vendor, so a machine without the file behaves as before.
- **Independence is by family, per issue.** The reviewer and the acceptance writer never share a family with the developer. Before anyone has developed, the acceptance writer avoids the family the developer is about to get, so the developer keeps its first choice.
- **One ledger for the machine.** A usage limit sets a provider and model aside in `~/.factory/availability.json`: 1 minute, then 5, 15, 30 and 60 for each further limit in a row. Every run reads it, so the next issue does not try them either.
- **Z.ai is read directly,** at most once a minute. The reading for this account: 5-hour window 1% used, weekly window 1% used, each with a reset time.
- **Parking.** When nobody may answer and the first comes back in more than 90 seconds, the run records a wake time and exits with code 75. It holds no slot while it waits. The queue goes on with other issues and starts it again at its wake time, at the step it stopped at.

## Problems hit

1. **The check reported the wrong cause.** For the 401 it showed the last line of the program's log, a note about an unrecognized model name. It now shows what the program reported.
2. **A scenario assumed which of two issues calls first.** The parked-queue scenario failed under load when the other issue was also caught by the limit. The stand-in now waits until the other issue's developer is at work.
3. **`zsh` does not split a variable into words,** so one batch of checks ran `claude --tier standard` as a single provider name and did nothing. Rerun.
4. **Two sessions edited the same checkout.** The provider registry and the first version of the check came from a forked session, uncommitted. They are in this commit.

## Open items

- **No real run with a providers file.** This machine has none in `~/.factory`. The checked trial file is in `~/.factory/trial/`.
- **Antigravity.** Its permission settings may be able to confine it. Not explored. Until something does, it cannot be used.
- **A reader with Antigravity would get the same sandbox as a writer.** The engine has no read-only mode for it. Moot while it fails the check.
- **Reset times in error messages are not read.** "Try again at 3:10 PM" is treated like any limit, with the fixed waits. Later the same day the engine began reading each plan's usage and reset times from the calls themselves: see [reading each plan's usage](2026-11-24-plan-usage.md).
- **The reader gap is unchanged.** Codex reads anywhere on the machine. The check reports it and does not fail it.
- **Model names are the user's.** `gpt-6.2-sol` and `gpt-6.2-astra` both answered. Which one is `high` is a choice the file makes, not the engine.
- **`factory run` alone parks and exits.** Only the queue comes back by itself.
