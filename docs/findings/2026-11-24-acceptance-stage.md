# Findings: the acceptance stage (2026-11-24)

Build-order step 3: acceptance writer, locked-flow gate, accessibility ID list, flow-change-request. Tried on `jaechoidev/todo-trial`, issue #5 ("Add a todo"), on a MacBook with Maestro 2.12.1, Xcode 27.2 and an iPhone 17 simulator on iOS 27.2.

## Result

`factory run 5` took the issue to [todo-trial#14](https://github.com/jaechoidev/todo-trial/pull/14) in 3 minutes 8 seconds.

| Step | Who | Result | Time |
| --- | --- | --- | --- |
| acceptance | Claude, may write only in the worktree | Wrote `.maestro/add_todo.yaml`, added the ID `todo.list.row` | 46 s |
| acceptance-files, flow-ids gates | engine | passed | under 1 s |
| red | `./verify.sh --red .maestro/add_todo.yaml` | The new flow fails on the old app, as it should | 40 s |
| develop | Codex, sandboxed to the worktree | 2 files changed | 44 s |
| locked-flows, scope, ids-implemented gates | engine | passed | under 1 s |
| verify | `./verify.sh`: build, then every flow | 2 of 2 flows passed | 38 s |
| review | Claude, read-only | approve, on the developer's commit | 12 s |

Codex could not run the simulator from its sandbox, so it wrote the feature without ever running the flow. The flow passed on the first try.

The first attempt at the same issue failed, which is the first real failing run. See "Problems hit".

## Answers to the design doc's open questions

| Question | Answer |
| --- | --- |
| Who rules on a flow-change-request by default | The acceptance writer, called again with the developer's reason. Accept means it edits the flow and the engine commits that separately. Reject stops the run until fix loops exist. Built, not yet exercised by a real run |
| How the acceptance writer chooses accessibility IDs consistently | The plan names the IDs. The writer uses them and may add more that follow the rule in `AGENTS.md`. The engine checks the format, comments the additions on the issue and hands the full list to the developer. In the trial the writer added `todo.list.row`, which follows the rule that repeated rows share one ID |

## How Maestro behaves

- `maestro test` exits 0 when every flow passes and 1 when any fails. A syntax error also exits 1, so a broken flow looks like a failing one. `maestro check-syntax` tells them apart, and `verify.sh --red` runs it first.
- `--format junit --output FILE` gives one result per flow. `verify.sh --red` reads it to confirm every named flow ran and none passed.
- One `maestro test` call costs about 20 seconds before the first command runs. A failing `assertVisible` waits about 17 seconds before it gives up. That is why the red check takes 40 seconds for one flow.
- `MAESTRO_CLI_NO_ANALYTICS=1` keeps runs quiet. `--analyze` is an AI feature and must stay out of `verify.sh`.
- Homebrew refuses to install anything while the Command Line Tools are a major version behind macOS. The Mac Studio needs the same check before Maestro is installed there.

## Other findings

- **Both agents caught the plan's mistake on their own.** The acceptance writer wrote in its notes that the scope probably needed `Sources/UI/List/`. The developer then refused to work outside the scope and said the same. The writer's warning came 60 seconds and one simulator run earlier, but the engine does not read notes.
- **The reviewer's items are about later issues, not this one.** In-memory storage and index-based row identity are fine for #5 and will matter for delete and persistence. Nothing carries them forward to those issues.
- **A rerun after a failure needs no cleanup when nothing was pushed.** The engine replaced the old worktree and branch and started again.
- **The CI review agreed, and could read the issue this time.** The `review` workflow on #14 commented "Verdict: approve" on the same commit and quoted the issue's widened scope, which exists only in the issue body. That confirms the `issues: read` fix from [todo-trial#12](https://github.com/jaechoidev/todo-trial/pull/12). It also read the PR body as contradicting itself: "verify passed" next to the developer's "verify was blocked". The body now says the passing run was the engine's.
- **`verify.sh` leaves the simulator booted.** That saves about 30 seconds on the next run. Nothing shuts it down.

## Where the build differs from the design doc

- **Accessibility IDs.** The doc has the acceptance writer choose them. Here the plan names them and the writer may add, because `plan.yaml` already carries an ID list per issue.
- **The red check is a mode of the repo's `verify.sh`.** The engine only knows `./verify.sh --red <flows>` and its exit code. The Maestro details stay in the repo.
- **An accepted flow change does not go back to the developer.** Verify and review run on the commit with the changed flow. A rejected one stops the run.
- **`flow_change_request` is free text,** not a structured object. Both CLIs already return it reliably as a nullable string.
- **Review does not explore the app.** The doc has the reviewer drive the app through XcodeBuildMCP and Maestro MCP to find gaps the flows miss. The reviewer here reads the code, the flows and the verify result.
- **Two more fixed gates than the doc lists.** A flow may not use AI or script commands, and every accessibility ID must appear as a string literal inside the issue's scope after development.

## Problems hit

1. **Issue #5's scope was too narrow.** The dummy plan gave it `Sources/Model/` and `Sources/UI/Add/`, but adding a todo needs the list screen. The first run stopped at develop with "the developer changed no files", labelled the issue `blocked`, and kept the worktree. The scope was widened by editing the issue by hand, and the second run passed.
2. **A plan can't be changed once its issues exist.** `plan create` finds an issue by plan and key and then leaves it alone. After the hand edit, `plans/todo-v1.yaml` on `main` and issue #5 disagree about the scope. Issues #6, #7 and #8 have the same kind of narrow scope.
3. **The developer never saw the acceptance writer's notes.** The writer listed five assumptions its flow makes. The developer passed anyway by reading the flow. The developer's prompt now includes those notes, added after the run and not yet exercised.
4. **A rerun commented the added IDs twice.** Fixed before the second run: the engine skips a comment that is already there.

## Open items

- **The flow-change ruling has never run for real.** The code path, its prompt and its gates are built. No developer has filed a request yet.
- **A flow can fail for the wrong reason and still pass the red check.** The ID gate catches a mistyped ID. It does not catch a flow whose first step can never work.
- **The developer works blind.** Its sandbox blocks the simulator, so a wrong first attempt can only be corrected by a fix loop, which is the next step.
- **The trial repo's `factory.yaml` on `dev` still has three steps.** The run used `--workflow` with the template's file.
- **Nothing carries a reviewer's or writer's remarks to later issues.**
- **One simulator, by name, with no lock.** Fine for one issue at a time.
