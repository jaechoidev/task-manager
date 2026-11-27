---
name: factory-plan
description: Turn a feature discussion, spec or plan document into a factory plan file and open it as a pull request into main. Use when the person says "turn this into a plan", "make this a plan PR", "write the plan for this", or names a spec or doc to plan from. This repo's issues are run by factory, so plans follow its format.
---

# Turn a feature into a factory plan

This repository is run by `factory`. A plan is one YAML file in `plans/`, merged into `main`. After the merge, the `plan-create` workflow makes one GitHub issue per entry, and the machine that runs the engine works on the ones the person labels `today`. Nothing in a plan can be changed after it merges, so get it right in the pull request.

The thinking about what to build can happen anywhere: in this conversation, with another planning skill, in a markdown file or a Claude doc. This skill is the last step: turning that into a plan that runs.

## Before writing

1. Read `plans/README.md` for the format and `AGENTS.md` for the accessibility ID rules. They are this repo's own copies and win over anything here.
2. Read the plans merged so far, which live on `main`: `git fetch origin && git ls-tree --name-only origin/main plans/`, then `git show origin/main:plans/<file>`. Read the code under `Sources/`, and `Tests/` if there is one, as it is on `origin/dev`, since scopes and IDs have to fit it.
3. Find the `factory` program: `command -v factory`, or `~/code/task-manager/target/release/factory`. If neither exists, say so and stop.

## Writing the plan

- **One issue is one change that can be tested and merged on its own.** Steps that can't land alone are acceptance criteria inside one issue. Prefer `size:S`. Those take about five minutes each.
- **Acceptance criteria are what a flow can check on screen.** A different agent turns each one into a Maestro flow before development, and the flow must fail on the app as it is. Write visible facts: exact text, counts, what a tap changes. Not taste ("looks clean"), not layout ("under the title"), which a flow can't check. A criterion with no UI, like storage, is checked by unit tests and says so.
- **`accessibility_ids` lists every ID the flows will use.** Development implements exactly those. Format from `AGENTS.md`: `<feature>.<screen>.<element>`, lowercase, words joined by `-`. Reuse an existing ID when the element exists already.
- **`flows`:** `{file: name.yaml, new: true}` for a flow this issue adds. Name it after the issue key.
- **Scope is every path development may change.** A change outside it fails the issue. When a change touches a model and a screen, list both folders. Never `.claude/`, `.github/` or `.maestro/`: the validator refuses the first two, and flows are written by the acceptance step.
- **When the repo has `Tests/`, each source folder brings its test folder.** For every `Sources/<area>/` in an issue's scope, also list `Tests/<area>/`, even if that folder doesn't exist yet. `Sources/Model/` goes with `Tests/Model/`. Without it, development can't add or update a test, so an issue that changes tested behaviour on purpose fails. Test folders mirror source folders, so this adds no new overlap and no new `depends_on`.
- **Two issues whose scopes overlap need `depends_on`** on one of them, or the validator refuses the plan. Issues with separate scopes run side by side.
- **Labels:** `needs-sim` for anything with a flow, `needs-research` when it needs an API or library the agents may not know, `priority:high` or `priority:low` only when the order matters.
- Plan ID: `<feature>-v<n>`, the file `plans/<plan-id>.yaml`, unused so far.

Show the person the plan in the conversation and change it until they are happy with it, before any pull request.

## Opening the pull request

Ask before this step, since it opens a pull request on GitHub.

```bash
git fetch origin
git switch -c plan/<plan-id> origin/main
```

Write `plans/<plan-id>.yaml`, then check it. Fix every error before going on:

```bash
factory plan validate plans/
```

```bash
git add plans/<plan-id>.yaml
git commit -m "plan: add <plan-id>"
git push -u origin plan/<plan-id>
gh pr create --base main --head plan/<plan-id> --title "plan: add <plan-id>" --body "<one line per issue: its key and what it adds>"
```

Then switch the checkout back to the branch it was on.

## After the pull request

- The `plan-validate` check comments the batches it will run. Read it, and tell the person the pull request link and the batches.
- **The person merges it.** `main` needs an approval the owner may bypass, and merging is how they approve the work. Merge it yourself only when the person says so in this conversation, for this pull request, then `gh pr merge <number> --merge --admin --match-head-commit <sha>`.
- After the merge, `plan-create` comments the issue numbers on the pull request. Tell the person, and that labelling them `today` starts the work. Don't label them yourself unless asked.
