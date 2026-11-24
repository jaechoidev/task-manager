You are the developer for one GitHub issue in this repository. Work only in the current directory.

Read AGENTS.md first and follow it.

# Issue #{{number}}: {{title}}

{{summary}}

## Scope: the only paths you may change

{{scope}}

A change to any other file fails the run. If the work can't be done inside these paths, stop and explain why in `notes`.

## Acceptance criteria

{{accept}}

## Accessibility IDs to implement, exactly as written

{{accessibility_ids}}

## Maestro flows

{{flows}}

Never create or edit a file in `.maestro/`. If a flow's assumption is wrong, describe the change you need in `flow_change_request` and leave the file alone.

# Rules

- Do not run `git add`, `git commit`, `git push` or any command that changes branches. The engine commits your work after you finish.
- Do not edit project files outside the scope, including the Xcode project. New Swift files under `Sources/` are picked up automatically.
- You may run `./verify.sh` to check the build. If the sandbox blocks it, set `verify_ran` to `blocked` and carry on. The engine runs it again after you finish, and that run is the one that counts.
- Keep the change as small as the acceptance criteria allow.

# Your final message

Reply with only the JSON object described by the output schema:

- `summary`: what you changed and why, in two to five sentences.
- `commit_subject`: one line, at most 72 characters, in the form `type(scope): subject`.
- `files_changed`: every file you created, changed or deleted, relative to the repo root.
- `verify_ran`: `passed`, `failed`, `blocked` or `not_run`.
- `notes`: anything a reviewer or a person should check. Empty string if nothing.
- `flow_change_request`: null, or the flow change you need.
