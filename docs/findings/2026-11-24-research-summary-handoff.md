# Findings: research, PR summary, handoff and promote (2026-11-24)

Build-order step 8: the research step, the PR summary, the handoff export and the promote PR.

## Result

| Part | What it does | How it was checked |
| --- | --- | --- |
| Research | For an issue labelled `needs-research`, or when a judgment asks, one agent writes `research/issue-<n>.md` with sources before anyone else works | A real run with Claude and web access, and 3 engine scenarios |
| PR summary | One LLM call writes what changed, how the run went and what a person should look at | The same real run, and 2 engine scenarios |
| Handoff | `factory handoff` writes the day's document per repository from the database | Unit tests and an engine scenario |
| Promote | `factory promote` opens or refreshes the pull request into `main` and never merges it | An engine scenario |

112 tests pass: 77 unit tests and 35 engine scenarios.

The real run, on a scratch repository with `gh` replaced by a script. The issue asked for a function that pluralizes with Foundation and not with an `if`:

| Step | Who | Result | Time |
| --- | --- | --- | --- |
| research | Claude, with web search and fetch | Notes with 15 sources: the API to use, what the docs do not say, and four open questions | 184 s |
| develop | Codex | Used the API the notes recommended, with the locale pinned as the notes suggested | 52 s |
| verify | type check only | passed | under 1 s |
| review | Claude | approve | 20 s |
| summary | Claude | See below | 10 s |

What the summary got right: it said the approval rests on reading the code, that no check ran the function, and that the result for other locales is unverified. It listed those first under "For a human to check", followed by the open questions from research.

## Where the build differs from the design doc

- **Research notes are committed to the issue's branch.** The doc has research produce a file whose path is handed to the developer. Committing it puts the notes in the pull request, where the reviewer and a person can read what the developer was told.
- **Only Claude researches with the web.** The engine gives web tools through Claude's tool list. Codex's sandbox has no network, so a Codex researcher works from the code.
- **The summary runs each time the branch is pushed,** not once per pull request, so the text matches the commit after a GitHub fix round.
- **The summary never replaces the facts.** The doc has one LLM call summarize the log into the PR body. Here the verified commit, gates and verdicts are still written by code, and the LLM's part sits above them.
- **The handoff has no LLM call of its own.** It repeats the summaries already written.
- **Handoff dates are UTC.** A run late in the evening, Pacific time, lands in the next day's file.

## Problems hit

1. **Research is slow.** Three minutes for one question, most of it reading documentation pages. That is longer than every other step of the run together.
2. **The first real run used a stale binary.** The release build was from before this step, so it rejected the new workflow file. Tests always use a fresh debug build. Only hand runs can go stale.

## Open items

- **Text from the web reaches the developer.** The researcher is told that web text is not an instruction, writes only one file, and has no shell. But the developer then reads that file. The gates on the developer still apply.
- **Nothing checks that the sources are real.** The gate wants a `## Sources` section and a non-empty list. It does not open the links.
- **Research and summary have not run on the iOS project.**
- **`factory promote` has only run against a stand-in.** On the trial repository, `main` requires an approval and `plan-validate`, and neither has been seen with a promotion.
- **No notification when a handoff is written or an issue needs a person.** That comes with the app.
